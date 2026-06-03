use crate::{
    ActError, KvStore, Result,
    utils::{consts, sync},
};
use sqlx::{Row, postgres::PgPoolOptions};

pub struct PostgresStore {
    pool: sqlx::PgPool,
}

impl PostgresStore {
    async fn init_pool(url: &str) -> Result<sqlx::PgPool> {
        let pool = PgPoolOptions::new()
            .min_connections(5)
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
        let pool = sync::block_on(Self::init_pool(url))?;
        Ok(Self { pool })
    }
}

impl KvStore for PostgresStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let key = key.to_string();
        sync::block_on(async {
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
        sync::block_on(async {
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
        sync::block_on(async {
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
        sync::block_on(async {
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
