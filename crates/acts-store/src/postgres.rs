use crate::consts;
use acts::{ActError, KvStore, Result, ScanOperation, ScanOptions, StoreBatchOp, StoreGuard};
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
                key TEXT COLLATE \"C\" PRIMARY KEY,
                value BYTEA NOT NULL
            )",
            consts::ACTS_STORE_NAME
        ))
        .execute(&pool)
        .await
        .map_err(|e| ActError::Store(e.to_string()))?;

        // The key column is a byte-ordered key space: index scans express
        // their half-open ranges `[lower, upper)` with separator bytes
        // (`KEY_SEP`, and `KEY_SEP_SUCC` as the exclusive bound) whose strict
        // byte ordering the range comparisons rely on. A locale collation
        // (e.g. en_US.UTF-8) gives those control bytes equal weights, so the
        // boundaries leak one row (an exclusive `>` includes the bound, an
        // inclusive upper excludes it). `COLLATE \"C\"` restores the byte
        // order the other backends (memory/sled/sqlite) compare in.
        // Pre-existing tables keep their original collation — recreate or
        // `ALTER TABLE ... ALTER COLUMN key TYPE text COLLATE \"C\"` once.

        Ok(Self { pool })
    }
}

#[async_trait::async_trait]
impl KvStore for PostgresStore {
    async fn one(&self, key: &str) -> Result<Option<Vec<u8>>> {
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

    async fn many(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }

        let mut values = Vec::with_capacity(keys.len());
        for chunk in keys.chunks(500) {
            let placeholders = (1..=chunk.len())
                .map(|n| format!("${n}"))
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT key, value FROM {} WHERE key IN ({})",
                consts::ACTS_STORE_NAME,
                placeholders
            );
            let mut query = sqlx::query_as::<_, (String, Vec<u8>)>(&sql);
            for key in chunk {
                query = query.bind(key);
            }
            let rows = query
                .fetch_all(&self.pool)
                .await
                .map_err(|e| ActError::Store(e.to_string()))?;
            let mut by_key = rows
                .into_iter()
                .collect::<std::collections::HashMap<_, _>>();
            values.extend(chunk.iter().map(|key| by_key.remove(key)));
        }
        Ok(values)
    }

    async fn batch(&self, ops: &[StoreBatchOp], guards: &[StoreGuard]) -> Result<bool> {
        if ops.is_empty() && guards.is_empty() {
            return Ok(true);
        }
        if guards.is_empty() && ops.len() == 1 {
            // A guardless single-key batch skips the BEGIN/COMMIT round trip.
            return match &ops[0] {
                StoreBatchOp::Put { key, value } => {
                    self.put(key, value.clone()).await.map(|()| true)
                }
                StoreBatchOp::Delete { key } => self.delete(key).await.map(|()| true),
            };
        }

        // One transaction for the guard reads and the ops, and each guard key
        // is locked before it is read:
        //
        // - an expected value locks the row (`FOR UPDATE`), so a concurrent
        //   write of the same key waits and then compares against the
        //   committed bytes — never against the snapshot it started with;
        // - an absent guard cannot lock a row that does not exist, so it takes
        //   a transaction-scoped advisory lock on the key instead, which is
        //   what serializes two creators of a key that is not there yet.
        //
        // Either way the comparison and the writes commit together, or the
        // transaction is dropped (rolled back) with nothing applied.
        let table = consts::ACTS_STORE_NAME;
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| ActError::Store(e.to_string()))?;
        for guard in guards {
            let row = if guard.expected.is_none() {
                sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
                    .bind(&guard.key)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| ActError::Store(e.to_string()))?;
                sqlx::query_as::<_, (Vec<u8>,)>(&format!(
                    "SELECT value FROM {} WHERE key = $1",
                    table
                ))
                .bind(&guard.key)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|e| ActError::Store(e.to_string()))?
            } else {
                sqlx::query_as::<_, (Vec<u8>,)>(&format!(
                    "SELECT value FROM {} WHERE key = $1 FOR UPDATE",
                    table
                ))
                .bind(&guard.key)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|e| ActError::Store(e.to_string()))?
            };
            if !guard.matches(row.as_ref().map(|(value,)| value.as_slice())) {
                // Dropping `tx` without committing rolls the whole batch back.
                return Ok(false);
            }
        }
        for op in ops {
            match op {
                StoreBatchOp::Put { key, value } => {
                    sqlx::query(&format!(
                        "INSERT INTO {} (key, value) VALUES ($1, $2)
                         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
                        table
                    ))
                    .bind(key)
                    .bind(value)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| ActError::Store(e.to_string()))?;
                }
                StoreBatchOp::Delete { key } => {
                    sqlx::query(&format!("DELETE FROM {} WHERE key = $1", table))
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
        Ok(true)
    }

    async fn scan_prefix(&self, key: &str, options: ScanOptions) -> Result<Vec<(String, Vec<u8>)>> {
        let ScanOptions {
            is_rev,
            op,
            ref prefix,
        } = options;
        let order = if is_rev { "DESC" } else { "ASC" };
        let mut sql = format!(
            "SELECT key, value FROM {} WHERE key LIKE $1 ESCAPE '\\'",
            consts::ACTS_STORE_NAME
        );
        let mut binds: Vec<String> = vec![like_pattern(prefix)];
        let mut param_idx = 2;
        match &op {
            ScanOperation::Eq => {
                let n = binds.len() + 1;
                sql.push_str(&format!(" AND key LIKE ${n} ESCAPE '\\'"));
                binds.push(like_pattern(key));
            }
            ScanOperation::Ne => {
                sql.push_str(&format!(" AND key NOT LIKE ${param_idx} ESCAPE '\\'"));
                binds.push(like_pattern(key));
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
                    sql.push_str(&format!("key LIKE ${param_idx} ESCAPE '\\'"));
                    binds.push(like_pattern(v));
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
