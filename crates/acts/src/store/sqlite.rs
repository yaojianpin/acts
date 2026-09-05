use crate::{
    ActError, KvStore, Result,
    store::{ScanOperation, ScanOptions, StoreBatchOp},
    utils::{consts, sync},
};
use parking_lot::Mutex;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, Row};
use std::sync::Arc;

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
        ScanOperation::Eq => {
            // keys LIKE 'key%' — handled by the LIKE pattern on prefix
            (String::new(), vec![])
        }
        ScanOperation::Ne => (" AND key NOT LIKE ?".to_string(), vec![format!("{}%", key)]),
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
            let mut conn = conn.lock();
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
            let mut conn = conn.lock();
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
            let mut conn = conn.lock();
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
    fn batch(&self, ops: &[StoreBatchOp]) -> Result<()> {
        if ops.is_empty() {
            return Ok(());
        }
        if ops.len() == 1 {
            // A single-key batch skips the BEGIN/COMMIT round trip.
            return match &ops[0] {
                StoreBatchOp::Put { key, value } => self.put(key, value.clone()),
                StoreBatchOp::Delete { key } => self.delete(key),
            };
        }
        let conn = self.conn.clone();
        sync::block_on(async move {
            let mut conn = conn.lock();
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
            let mut conn = conn.lock();
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
