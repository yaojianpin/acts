use crate::{
    ActError, KvStore, Result,
    store::{ScanOperation, ScanOptions},
    utils::{consts, sync},
};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, Row};
use std::sync::{Arc, Mutex};

pub struct SqliteStore {
    conn: Arc<Mutex<sqlx::SqliteConnection>>,
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
        Ok(conn)
    }

    pub fn open(path: &str) -> Result<Self> {
        let conn = sync::block_on(Self::init_conn(path))?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    #[allow(dead_code)]
    pub fn open_in_memory() -> Result<Self> {
        Self::open(":memory:")
    }
}

/// Build extra WHERE conditions for scan operations.
fn op_conditions(op: &ScanOperation, key: &str) -> (String, Vec<String>) {
    match op {
        ScanOperation::Eq | ScanOperation::Match => {
            // keys LIKE 'key%' — handled by the LIKE pattern on prefix
            (String::new(), vec![])
        }
        ScanOperation::Gt => (" AND key > ?".to_string(), vec![key.to_string()]),
        ScanOperation::Ge => (" AND key >= ?".to_string(), vec![key.to_string()]),
        ScanOperation::Lt => (" AND key < ?".to_string(), vec![key.to_string()]),
        ScanOperation::Le => (" AND key <= ?".to_string(), vec![key.to_string()]),
        ScanOperation::Ne => (" AND key NOT LIKE ?".to_string(), vec![format!("{}%", key)]),
        ScanOperation::Range { from, to } => {
            let start = format!("{}{}", key, from);
            let end = format!("{}{}", key, to);
            (" AND key >= ? AND key < ?".to_string(), vec![start, end])
        }
        ScanOperation::ExclusiveRange { from, to } => {
            let start = format!("{}{}", key, from);
            let end = format!("{}{}", key, to);
            (" AND key > ? AND key < ?".to_string(), vec![start, end])
        }
        ScanOperation::InclusiveRange { from, to } => {
            let start = format!("{}{}", key, from);
            let end = format!("{}{}", key, to);
            (" AND key >= ? AND key <= ?".to_string(), vec![start, end])
        }
        ScanOperation::In { values } => {
            let mut conditions = String::from(" AND (key LIKE ?");
            for _ in 1..values.len() {
                conditions.push_str(" OR key LIKE ?");
            }
            conditions.push(')');
            let binds: Vec<String> = values.iter().map(|v| format!("{}%", v)).collect();
            (conditions, binds)
        }
    }
}

impl KvStore for SqliteStore {
    #[allow(clippy::await_holding_lock)]
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let key = key.to_string();
        let conn = self.conn.clone();
        sync::block_on(async move {
            let mut conn = conn.lock().unwrap();
            sqlx::query(&format!(
                "SELECT value FROM {} WHERE key = ?",
                consts::ACTS_STORE_NAME
            ))
            .bind(&key)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| ActError::Store(e.to_string()))
            .map(|opt| opt.map(|row| row.get(0)))
        })
    }

    #[allow(clippy::await_holding_lock)]
    fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        let key = key.to_string();
        let conn = self.conn.clone();
        sync::block_on(async move {
            let mut conn = conn.lock().unwrap();
            sqlx::query(&format!(
                "INSERT INTO {} (key, value) VALUES (?, ?)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                consts::ACTS_STORE_NAME
            ))
            .bind(&key)
            .bind(&value)
            .execute(&mut *conn)
            .await
            .map_err(|e| ActError::Store(e.to_string()))?;
            Ok(())
        })
    }

    #[allow(clippy::await_holding_lock)]
    fn delete(&self, key: &str) -> Result<()> {
        let key = key.to_string();
        let conn = self.conn.clone();
        sync::block_on(async move {
            let mut conn = conn.lock().unwrap();
            sqlx::query(&format!(
                "DELETE FROM {} WHERE key = ?",
                consts::ACTS_STORE_NAME
            ))
            .bind(&key)
            .execute(&mut *conn)
            .await
            .map_err(|e| ActError::Store(e.to_string()))?;
            Ok(())
        })
    }

    #[allow(clippy::await_holding_lock)]
    fn scan_prefix(&self, key: &str, options: ScanOptions) -> Result<Vec<(String, Vec<u8>)>> {
        let ScanOptions {
            is_rev,
            op,
            ref prefix,
        } = options;
        let pattern = format!("{}%", prefix);
        let (extra_sql, extra_binds) = op_conditions(&op, key);
        let conn = self.conn.clone();
        sync::block_on(async move {
            let mut conn = conn.lock().unwrap();
            let order = if is_rev { "DESC" } else { "ASC" };
            let sql = format!(
                "SELECT key, value FROM {} WHERE key LIKE ?{} ORDER BY key {}",
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
        })
    }
}
