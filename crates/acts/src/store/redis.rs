use crate::{
    ActError, KvStore, Result,
    store::{ScanOperation, ScanOptions, StoreBatchOp},
};
use redis::aio::MultiplexedConnection;
use redis::{AsyncCommands, Client};

pub struct RedisStore {
    conn: MultiplexedConnection,
}

impl RedisStore {
    pub async fn open(url: &str) -> Result<Self> {
        let client = Client::open(url).map_err(|e| ActError::Store(e.to_string()))?;
        let conn = client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| ActError::Store(e.to_string()))?;
        Ok(Self { conn })
    }
}

/// Return true if `k` matches the scan operation given `key` and `prefix`.
fn key_matches(k: &str, key: &str, prefix: &str, op: &ScanOperation) -> bool {
    if !k.starts_with(prefix) {
        return false;
    }
    match op {
        ScanOperation::Eq => k.starts_with(key),
        ScanOperation::Ne => !k.starts_with(key),
        ScanOperation::In { values } => values.iter().any(|v| k.starts_with(v.as_str())),
        ScanOperation::Range { lower, upper } => {
            if let Some(l) = lower
                && k < l.as_str()
            {
                return false;
            }
            if let Some(u) = upper
                && k >= u.as_str()
            {
                return false;
            }
            true
        }
    }
}

#[async_trait::async_trait]
impl KvStore for RedisStore {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let mut conn = self.conn.clone();
        conn.get(key)
            .await
            .map_err(|e| ActError::Store(e.to_string()))
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        let mut conn = self.conn.clone();
        conn.set(key, value)
            .await
            .map_err(|e| ActError::Store(e.to_string()))
    }

    async fn delete(&self, key: &str) -> Result<()> {
        let mut conn = self.conn.clone();
        conn.del(key)
            .await
            .map_err(|e| ActError::Store(e.to_string()))
    }

    async fn batch(&self, ops: &[StoreBatchOp]) -> Result<()> {
        if ops.is_empty() {
            return Ok(());
        }
        if ops.len() == 1 {
            // A single-key batch skips the MULTI/EXEC round trip.
            return match &ops[0] {
                StoreBatchOp::Put { key, value } => self.put(key, value.clone()).await,
                StoreBatchOp::Delete { key } => self.delete(key).await,
            };
        }
        // `atomic()` runs the commands through MULTI/EXEC on the single
        // connection: everything is queued, then executed together, so no
        // other client can observe a partially applied batch.
        let mut conn = self.conn.clone();
        let mut pipe = redis::pipe();
        pipe.atomic();
        for op in ops {
            match op {
                StoreBatchOp::Put { key, value } => {
                    pipe.cmd("SET").arg(key.as_str()).arg(value.as_slice());
                }
                StoreBatchOp::Delete { key } => {
                    pipe.cmd("DEL").arg(key.as_str());
                }
            }
        }
        pipe.query_async::<()>(&mut conn)
            .await
            .map_err(|e| ActError::Store(e.to_string()))
    }

    async fn scan_prefix(&self, key: &str, options: ScanOptions) -> Result<Vec<(String, Vec<u8>)>> {
        let ScanOptions {
            is_rev,
            op,
            ref prefix,
        } = options;
        let mut conn = self.conn.clone();
        let pattern = format!("{}*", prefix);
        let mut result = Vec::new();
        let mut cursor: String = "0".to_string();
        loop {
            let (next_cursor, keys): (String, Vec<String>) = redis::cmd("SCAN")
                .arg(&cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(100)
                .query_async(&mut conn)
                .await
                .map_err(|e| ActError::Store(e.to_string()))?;
            for key_str in keys {
                if !key_matches(&key_str, key, prefix, &op) {
                    continue;
                }
                let val: Option<Vec<u8>> = conn
                    .get(&key_str)
                    .await
                    .map_err(|e| ActError::Store(e.to_string()))?;
                if let Some(v) = val {
                    result.push((key_str, v));
                }
            }
            if next_cursor == "0" {
                break;
            }
            cursor = next_cursor;
        }
        if is_rev {
            result.reverse();
        }
        Ok(result)
    }
}
