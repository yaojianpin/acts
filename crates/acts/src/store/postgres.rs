use crate::{
    ActError, KvStore, Result,
    store::{ScanOperation, ScanOptions, StoreBatchOp},
    utils::consts,
};
use sqlx::{Row, postgres::PgPoolOptions};
use std::time::Duration;

pub struct PostgresStore {
    pool: sqlx::PgPool,
}

impl PostgresStore {
    pub async fn open(url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .min_connections(1)
            .max_connections(50)
            .acquire_timeout(Duration::from_secs(60))
            .connect(url)
            .await
            .map_err(|e| ActError::Store(e.to_string()))?;

        sqlx::query(&format!(
            "CREATE TABLE IF NOT EXISTS {0} (
                key TEXT PRIMARY KEY,
                value BYTEA NOT NULL
            )",
            consts::ACTS_STORE_NAME
        ))
        .execute(&pool)
        .await
        .map_err(|e| ActError::Store(e.to_string()))?;

        Ok(Self { pool })
    }
}

#[async_trait::async_trait]
impl KvStore for PostgresStore {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        sqlx::query(&format!(
            "SELECT value FROM {} WHERE key = $1",
            consts::ACTS_STORE_NAME
        ))
        .bind(key)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| ActError::Store(e.to_string()))
        .map(|opt| opt.map(|row| row.get(0)))
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        sqlx::query(&format!(
            "INSERT INTO {} (key, value) VALUES ($1, $2)
             ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
            consts::ACTS_STORE_NAME
        ))
        .bind(key)
        .bind(&value)
        .execute(&self.pool)
        .await
        .map_err(|e| ActError::Store(e.to_string()))?;
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<()> {
        sqlx::query(&format!(
            "DELETE FROM {} WHERE key = $1",
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
        // One connection for the whole batch: `tx` commits everything or,
        // when an op fails and the method returns early, is dropped and
        // rolls the whole batch back.
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| ActError::Store(e.to_string()))?;
        for op in ops {
            match op {
                StoreBatchOp::Put { key, value } => {
                    sqlx::query(&format!(
                        "INSERT INTO {} (key, value) VALUES ($1, $2)
                         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
                        consts::ACTS_STORE_NAME
                    ))
                    .bind(key)
                    .bind(value)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| ActError::Store(e.to_string()))?;
                }
                StoreBatchOp::Delete { key } => {
                    sqlx::query(&format!(
                        "DELETE FROM {} WHERE key = $1",
                        consts::ACTS_STORE_NAME
                    ))
                    .bind(key)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| ActError::Store(e.to_string()))?;
                }
            }
        }
        tx.commit()
            .await
            .map_err(|e| ActError::Store(e.to_string()))?;
        Ok(())
    }

    async fn scan_prefix(&self, key: &str, options: ScanOptions) -> Result<Vec<(String, Vec<u8>)>> {
        let ScanOptions {
            is_rev,
            op,
            ref prefix,
        } = options;
        let pattern = format!("{}%", prefix);
        let order = if is_rev { "DESC" } else { "ASC" };
        let mut sql = format!(
            "SELECT key, value FROM {} WHERE key LIKE $1",
            consts::ACTS_STORE_NAME
        );
        let mut binds: Vec<String> = vec![pattern];
        let mut param_idx = 2;
        match &op {
            ScanOperation::Eq => {}
            ScanOperation::Ne => {
                sql.push_str(&format!(" AND key NOT LIKE ${}", param_idx));
                binds.push(format!("{}%", key));
            }
            ScanOperation::Range { lower, upper } => {
                if let Some(l) = lower {
                    let n = binds.len() + 1;
                    sql.push_str(&format!(" AND key >= ${}", n));
                    binds.push(l.clone());
                }
                if let Some(u) = upper {
                    let n = binds.len() + 1;
                    sql.push_str(&format!(" AND key < ${}", n));
                    binds.push(u.clone());
                }
            }
            ScanOperation::In { values } => {
                sql.push_str(" AND (");
                for (i, v) in values.iter().enumerate() {
                    if i > 0 {
                        sql.push_str(" OR ");
                    }
                    sql.push_str(&format!("key LIKE ${}", param_idx));
                    binds.push(format!("{}%", v));
                    param_idx += 1;
                }
                sql.push(')');
            }
        }
        sql.push_str(&format!(" ORDER BY key {}", order));
        let mut query = sqlx::query_as::<_, (String, Vec<u8>)>(&sql);
        for bind_val in &binds {
            query = query.bind(bind_val);
        }
        let rows = query
            .fetch_all(&self.pool)
            .await
            .map_err(|e| ActError::Store(e.to_string()))?;
        Ok(rows)
    }
}
