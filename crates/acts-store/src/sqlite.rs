use crate::consts;
use acts::{ActError, KvStore, Result, ScanOperation, ScanOptions, StoreBatchOp};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, Row};
use tokio::sync::Mutex;

pub struct SqliteStore {
    conn: Mutex<sqlx::SqliteConnection>,
}

impl SqliteStore {
    async fn init_conn(path: &str) -> Result<sqlx::SqliteConnection> {
        let opts = if path == ":memory:" {
            SqliteConnectOptions::new()
                .filename(":memory:")
                .create_if_missing(true)
        } else {
            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true)
        };
        let mut conn = sqlx::SqliteConnection::connect_with(&opts)
            .await
            .map_err(|e| ActError::Store(e.to_string()))?;
        sqlx::query(&format!(
            "CREATE TABLE IF NOT EXISTS {} (
                    key TEXT PRIMARY KEY,
                    value BLOB NOT NULL
                )",
            consts::ACTS_STORE_NAME
        ))
        .execute(&mut conn)
        .await
        .map_err(|e| ActError::Store(e.to_string()))?;
        sqlx::query("PRAGMA case_sensitive_like = ON")
            .execute(&mut conn)
            .await
            .map_err(|e| ActError::Store(e.to_string()))?;
        Ok(conn)
    }

    pub async fn open(path: &str) -> Result<Self> {
        let conn = Self::init_conn(path).await?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    #[allow(dead_code)]
    pub async fn open_in_memory() -> Result<Self> {
        Self::open(":memory:").await
    }
}

/// Build extra WHERE conditions for scan operations.
fn op_conditions(op: &ScanOperation, key: &str) -> (String, Vec<String>) {
    match op {
        ScanOperation::Eq => {
            // `key` is the exact value-key prefix (including the field
            // prefix), so push it into SQLite instead of scanning the whole
            // field group and filtering in the collection layer.
            (
                " AND key LIKE ? ESCAPE '\\'".to_string(),
                vec![like_pattern(key)],
            )
        }
        ScanOperation::Ne => (
            " AND key NOT LIKE ? ESCAPE '\\'".to_string(),
            vec![like_pattern(key)],
        ),
        ScanOperation::Range { lower, upper } => {
            let mut conditions = String::new();
            let mut binds = Vec::new();
            if let Some(l) = lower {
                conditions.push_str(" AND key >= ?");
                binds.push(l.clone());
            }
            if let Some(u) = upper {
                conditions.push_str(" AND key < ?");
                binds.push(u.clone());
            }
            (conditions, binds)
        }
        ScanOperation::In { values } => {
            let mut conditions = String::from(" AND (key LIKE ? ESCAPE '\\'");
            for _ in 1..values.len() {
                conditions.push_str(" OR key LIKE ? ESCAPE '\\'");
            }
            conditions.push(')');
            let binds: Vec<String> = values.iter().map(|v| like_pattern(v)).collect();
            (conditions, binds)
        }
    }
}

/// Turn a key prefix into a literal LIKE prefix. Values are encoded before
/// they enter index keys, but escaping keeps direct callers and field prefixes
/// safe if they contain SQL LIKE wildcards.
fn like_pattern(prefix: &str) -> String {
    let mut pattern = String::with_capacity(prefix.len() + 1);
    for ch in prefix.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(ch);
    }
    pattern.push('%');
    pattern
}

#[async_trait::async_trait]
impl KvStore for SqliteStore {
    #[allow(clippy::await_holding_lock)]
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let mut conn = self.conn.lock().await;
        sqlx::query(&format!(
            "SELECT value FROM {} WHERE key = ?",
            consts::ACTS_STORE_NAME
        ))
        .bind(key)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| ActError::Store(e.to_string()))
        .map(|opt| opt.map(|row| row.get(0)))
    }

    #[allow(clippy::await_holding_lock)]
    async fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        let mut conn = self.conn.lock().await;
        sqlx::query(&format!(
            "INSERT INTO {} (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            consts::ACTS_STORE_NAME
        ))
        .bind(key)
        .bind(&value)
        .execute(&mut *conn)
        .await
        .map_err(|e| ActError::Store(e.to_string()))?;
        Ok(())
    }

    #[allow(clippy::await_holding_lock)]
    async fn delete(&self, key: &str) -> Result<()> {
        let mut conn = self.conn.lock().await;
        sqlx::query(&format!(
            "DELETE FROM {} WHERE key = ?",
            consts::ACTS_STORE_NAME
        ))
        .bind(key)
        .execute(&mut *conn)
        .await
        .map_err(|e| ActError::Store(e.to_string()))?;
        Ok(())
    }

    #[allow(clippy::await_holding_lock)]
    async fn batch(&self, ops: &[StoreBatchOp]) -> Result<()> {
        if ops.is_empty() {
            return Ok(());
        }
        if ops.len() == 1 {
            // A single-key batch skips the BEGIN/COMMIT round trip.
            return match &ops[0] {
                StoreBatchOp::Put { key, value } => self.put(key, value.clone()).await,
                StoreBatchOp::Delete { key } => self.delete(key).await,
            };
        }
        let mut conn = self.conn.lock().await;
        let table = consts::ACTS_STORE_NAME;
        let res: std::result::Result<(), sqlx::Error> = async {
            sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
            for op in ops {
                match op {
                    StoreBatchOp::Put { key, value } => {
                        sqlx::query(&format!(
                            "INSERT INTO {} (key, value) VALUES (?, ?)
                             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                            table
                        ))
                        .bind(key)
                        .bind(value)
                        .execute(&mut *conn)
                        .await?;
                    }
                    StoreBatchOp::Delete { key } => {
                        sqlx::query(&format!("DELETE FROM {} WHERE key = ?", table))
                            .bind(key)
                            .execute(&mut *conn)
                            .await?;
                    }
                }
            }
            sqlx::query("COMMIT").execute(&mut *conn).await?;
            Ok(())
        }
        .await;
        match res {
            Ok(()) => Ok(()),
            Err(err) => {
                // Roll back the failed batch: without this the partial
                // writes would stay in the open transaction, invisible
                // to readers but never committed.
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                Err(ActError::Store(err.to_string()))
            }
        }
    }

    #[allow(clippy::await_holding_lock)]
    async fn scan_prefix(&self, key: &str, options: ScanOptions) -> Result<Vec<(String, Vec<u8>)>> {
        let ScanOptions {
            is_rev,
            op,
            ref prefix,
        } = options;
        let pattern = like_pattern(prefix);
        let (extra_sql, extra_binds) = op_conditions(&op, key);
        let mut conn = self.conn.lock().await;
        let order = if is_rev { "DESC" } else { "ASC" };
        let sql = format!(
            "SELECT key, value FROM {} WHERE key LIKE ? ESCAPE '\\'{} ORDER BY key {}",
            consts::ACTS_STORE_NAME,
            extra_sql,
            order
        );
        let mut query = sqlx::query(&sql).bind(&pattern);
        for bind_val in &extra_binds {
            query = query.bind(bind_val);
        }
        let rows = query
            .fetch_all(&mut *conn)
            .await
            .map_err(|e| ActError::Store(e.to_string()))?;
        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            let key: String = row.get(0);
            let value: Vec<u8> = row.get(1);
            result.push((key, value));
        }
        Ok(result)
    }
}
