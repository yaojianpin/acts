use crate::{
    ActError, KvStore, Result,
    store::{ScanOperation, ScanOptions},
    utils::{consts, sync},
};
use sqlx::{Row, postgres::PgPoolOptions};
use std::sync::OnceLock;
use std::time::Duration;

static POOL: OnceLock<sqlx::PgPool> = OnceLock::new();

fn pool() -> &'static sqlx::PgPool {
    POOL.get().expect("Postgres pool not initialized")
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

    fn scan_prefix(&self, key: &str, options: ScanOptions) -> Result<Vec<(String, Vec<u8>)>> {
        let ScanOptions {
            is_rev,
            op,
            ref prefix,
        } = options;
        let pattern = format!("{}%", prefix);
        sync::block_on(async move {
            let order = if is_rev { "DESC" } else { "ASC" };
            let mut sql = format!(
                "SELECT key, value FROM {} WHERE key LIKE $1",
                consts::ACTS_STORE_NAME
            );
            let mut binds: Vec<String> = vec![pattern];
            let mut param_idx = 2;
            match &op {
                ScanOperation::Eq | ScanOperation::Match => {}
                ScanOperation::Gt => {
                    sql.push_str(&format!(" AND key > ${}", param_idx));
                    binds.push(key.to_string());
                }
                ScanOperation::Ge => {
                    sql.push_str(&format!(" AND key >= ${}", param_idx));
                    binds.push(key.to_string());
                }
                ScanOperation::Lt => {
                    sql.push_str(&format!(" AND key < ${}", param_idx));
                    binds.push(key.to_string());
                }
                ScanOperation::Le => {
                    sql.push_str(&format!(" AND key <= ${}", param_idx));
                    binds.push(key.to_string());
                }
                ScanOperation::Ne => {
                    sql.push_str(&format!(" AND key NOT LIKE ${}", param_idx));
                    binds.push(format!("{}%", key));
                }
                ScanOperation::Range { from, to } => {
                    let start = format!("{}{}", key, from);
                    let end = format!("{}{}", key, to);
                    sql.push_str(&format!(
                        " AND key >= ${} AND key < ${}",
                        param_idx,
                        param_idx + 1
                    ));
                    binds.push(start);
                    binds.push(end);
                }
                ScanOperation::ExclusiveRange { from, to } => {
                    let start = format!("{}{}", key, from);
                    let end = format!("{}{}", key, to);
                    sql.push_str(&format!(
                        " AND key > ${} AND key < ${}",
                        param_idx,
                        param_idx + 1
                    ));
                    binds.push(start);
                    binds.push(end);
                }
                ScanOperation::InclusiveRange { from, to } => {
                    let start = format!("{}{}", key, from);
                    let end = format!("{}{}", key, to);
                    sql.push_str(&format!(
                        " AND key >= ${} AND key <= ${}",
                        param_idx,
                        param_idx + 1
                    ));
                    binds.push(start);
                    binds.push(end);
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
                .fetch_all(pool())
                .await
                .map_err(|e| ActError::Store(e.to_string()))?;
            Ok(rows)
        })
    }
}
