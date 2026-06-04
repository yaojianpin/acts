use crate::{
    ActError, KvStore, Result,
    utils::{consts, sync},
};
use sqlx::{Row, postgres::PgPoolOptions};
use std::sync::OnceLock;
use std::time::Duration;

static POOL: OnceLock<sqlx::PgPool> = OnceLock::new();

fn pool() -> &'static sqlx::PgPool {
    POOL.get().expect("Postgres pool not initialized")
}

/// Escape special characters in a string for SQL LIKE pattern matching.
/// Escapes backslash, percent, and underscore to prevent them from being
/// treated as wildcards.
fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

pub struct PostgresStore;

impl PostgresStore {
    pub fn open(url: &str) -> Result<Self> {
        init_pool(url)?;
        Ok(Self)
    }
}

fn init_pool(url: &str) -> Result<()> {
    if POOL.get().is_some() {
        return Ok(());
    }

    let url = url.to_string();
    let pool = sync::block_on(async move {
        let pool = PgPoolOptions::new()
            .min_connections(1)
            .max_connections(50)
            .acquire_timeout(Duration::from_secs(60))
            .connect(&url)
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

        // Truncate to remove accumulated data from previous test runs
        sqlx::query(&format!("TRUNCATE TABLE {}", consts::ACTS_STORE_NAME))
            .execute(&pool)
            .await
            .map_err(|e| ActError::Store(e.to_string()))?;

        Ok::<_, ActError>(pool)
    })?;

    // If another thread raced and already set the pool, that's fine — drop ours
    let _ = POOL.set(pool);
    Ok(())
}

impl KvStore for PostgresStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let key = key.to_string();
        sync::block_on(async move {
            sqlx::query(&format!(
                "SELECT value FROM {} WHERE key = $1",
                consts::ACTS_STORE_NAME
            ))
            .bind(&key)
            .fetch_optional(pool())
            .await
            .map_err(|e| ActError::Store(e.to_string()))
            .map(|opt| opt.map(|row| row.get(0)))
        })
    }

    fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        let key = key.to_string();
        sync::block_on(async move {
            sqlx::query(&format!(
                "INSERT INTO {} (key, value) VALUES ($1, $2)
                 ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
                consts::ACTS_STORE_NAME
            ))
            .bind(&key)
            .bind(&value)
            .execute(pool())
            .await
            .map_err(|e| ActError::Store(e.to_string()))?;
            Ok(())
        })
    }

    fn delete(&self, key: &str) -> Result<()> {
        let key = key.to_string();
        sync::block_on(async move {
            sqlx::query(&format!(
                "DELETE FROM {} WHERE key = $1",
                consts::ACTS_STORE_NAME
            ))
            .bind(&key)
            .execute(pool())
            .await
            .map_err(|e| ActError::Store(e.to_string()))?;
            Ok(())
        })
    }

    fn scan_prefix(&self, prefix: &str, is_rev: bool) -> Result<Vec<(String, Vec<u8>)>> {
        let pattern = format!("{}%", escape_like(prefix));
        sync::block_on(async move {
            let order = if is_rev { "DESC" } else { "ASC" };
            let rows = sqlx::query(&format!(
                "SELECT key, value FROM {} WHERE key LIKE $1 ORDER BY key {}",
                consts::ACTS_STORE_NAME, order
            ))
            .bind(&pattern)
            .fetch_all(pool())
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
