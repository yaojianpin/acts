use std::future::Future;

use sqlx::Row;
use sqlx::postgres::PgPoolOptions;
use tokio::runtime::Runtime;

use crate::{ActError, Result, utils::consts};

use super::kv::KvStore;

pub struct PostgresStore {
    pool: sqlx::PgPool,
    runtime: Option<Runtime>,
}

impl PostgresStore {
    async fn init_pool(url: &str) -> Result<sqlx::PgPool> {
        let pool = PgPoolOptions::new()
            .min_connections(1)
            .max_connections(100)
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
        Ok(pool)
    }

    pub fn open(url: &str) -> Result<Self> {
        let url = url.to_string();
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                let pool = tokio::task::block_in_place(|| handle.block_on(Self::init_pool(&url)))?;
                Ok(Self {
                    pool,
                    runtime: None,
                })
            }
            Err(_) => {
                let runtime = Runtime::new().map_err(|e| ActError::Store(e.to_string()))?;
                let pool = runtime.block_on(Self::init_pool(&url))?;
                Ok(Self {
                    pool,
                    runtime: Some(runtime),
                })
            }
        }
    }

    fn block_on<F: Future>(&self, f: F) -> F::Output {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(move || handle.block_on(f)),
            Err(_) => self
                .runtime
                .as_ref()
                .expect("no runtime available")
                .block_on(f),
        }
    }
}

impl KvStore for PostgresStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let key = key.to_string();
        self.block_on(async {
            sqlx::query(&format!(
                "SELECT value FROM {} WHERE key = $1",
                consts::ACTS_STORE_NAME
            ))
            .bind(&key)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| ActError::Store(e.to_string()))
            .map(|opt| opt.map(|row| row.get(0)))
        })
    }

    fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        let key = key.to_string();
        self.block_on(async {
            sqlx::query(&format!(
                "INSERT INTO {} (key, value) VALUES ($1, $2)
                 ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
                consts::ACTS_STORE_NAME
            ))
            .bind(&key)
            .bind(&value)
            .execute(&self.pool)
            .await
            .map_err(|e| ActError::Store(e.to_string()))?;
            Ok(())
        })
    }

    fn delete(&self, key: &str) -> Result<()> {
        let key = key.to_string();
        self.block_on(async {
            sqlx::query(&format!(
                "DELETE FROM {} WHERE key = $1",
                consts::ACTS_STORE_NAME
            ))
            .bind(&key)
            .execute(&self.pool)
            .await
            .map_err(|e| ActError::Store(e.to_string()))?;
            Ok(())
        })
    }

    fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>> {
        let pattern = format!("{}%", prefix);
        self.block_on(async {
            let rows = sqlx::query(&format!(
                "SELECT key, value FROM {} WHERE key LIKE $1",
                consts::ACTS_STORE_NAME
            ))
            .bind(&pattern)
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
        })
    }
}
