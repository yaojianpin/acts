use crate::consts;
use acts::{ActError, KvStore, Result, ScanOperation, ScanOptions, StoreBatchOp};
use std::time::Duration;

use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

pub struct SqliteStore {
    pool: sqlx::SqlitePool,
}

impl SqliteStore {
    async fn init_pool(path: &str) -> Result<sqlx::SqlitePool> {
        let is_memory = path == ":memory:";
        let mut opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .busy_timeout(Duration::from_secs(5));

        // WAL lets multiple pool connections read while a write is in
        // progress. SQLite still serializes writers internally, which is the
        // consistency model expected by this store.
        if !is_memory {
            opts = opts.journal_mode(SqliteJournalMode::Wal);
        }

        // In-memory databases are private to their connection. Keep one
        // pooled connection so all operations share the same database.
        let max_connections = if is_memory { 1 } else { 4 };
        let table = consts::ACTS_STORE_NAME;
        let pool = SqlitePoolOptions::new()
            .max_connections(max_connections)
            .after_connect(move |conn, _meta| {
                let table = table;
                Box::pin(async move {
                    sqlx::query(&format!(
                        "CREATE TABLE IF NOT EXISTS {table} (
                            key TEXT PRIMARY KEY,
                            value BLOB NOT NULL
                        )"
                    ))
                    .execute(&mut *conn)
                    .await?;
                    sqlx::query("PRAGMA case_sensitive_like = ON")
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .connect_with(opts)
            .await
            .map_err(|e| ActError::Store(e.to_string()))?;

        Ok(pool)
    }

    pub async fn open(path: &str) -> Result<Self> {
        let pool = Self::init_pool(path).await?;
        Ok(Self { pool })
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
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        sqlx::query(&format!(
            "SELECT value FROM {} WHERE key = ?",
            consts::ACTS_STORE_NAME
        ))
        .bind(key)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| ActError::Store(e.to_string()))?
        .map(|row| row.get::<Vec<u8>, _>(0))
        .map(Ok)
        .transpose()
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        sqlx::query(&format!(
            "INSERT INTO {} (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            consts::ACTS_STORE_NAME
        ))
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await
        .map_err(|e| ActError::Store(e.to_string()))?;
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<()> {
        sqlx::query(&format!(
            "DELETE FROM {} WHERE key = ?",
            consts::ACTS_STORE_NAME
        ))
        .bind(key)
        .execute(&self.pool)
        .await
        .map_err(|e| ActError::Store(e.to_string()))?;
        Ok(())
    }

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

        // Use an explicit immediate transaction so the batch waits for any
        // active writer before taking SQLite's write lock. The pooled
        // connection is returned on commit or rollback.
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| ActError::Store(e.to_string()))?;
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

    async fn scan_prefix(&self, key: &str, options: ScanOptions) -> Result<Vec<(String, Vec<u8>)>> {
        let ScanOptions {
            is_rev,
            op,
            ref prefix,
        } = options;
        let pattern = like_pattern(prefix);
        let (extra_sql, extra_binds) = op_conditions(&op, key);
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
            .fetch_all(&self.pool)
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
