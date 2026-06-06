use crate::{
    ActError, KvStore, Result,
    store::{ScanOperation, ScanOptions},
};
use redis::{Client, Commands};
use std::sync::Mutex;

pub struct RedisStore {
    conn: Mutex<redis::Connection>,
}

impl RedisStore {
    pub fn open(url: &str) -> Result<Self> {
        let client = Client::open(url).map_err(|e| ActError::Store(e.to_string()))?;
        let conn = client
            .get_connection()
            .map_err(|e| ActError::Store(e.to_string()))?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
}

/// Return true if `k` matches the scan operation given `key` and `prefix`.
fn key_matches(k: &str, key: &str, prefix: &str, op: &ScanOperation) -> bool {
    if !k.starts_with(prefix) {
        return false;
    }
    match op {
        ScanOperation::Eq | ScanOperation::Match => k.starts_with(key),
        ScanOperation::Gt => k > key,
        ScanOperation::Ge => k >= key,
        ScanOperation::Lt => k < key,
        ScanOperation::Le => k <= key,
        ScanOperation::Ne => !k.starts_with(key),
        ScanOperation::Range { from, to } => {
            let start = format!("{}{}", key, from);
            let end = format!("{}{}", key, to);
            k >= start.as_str() && k < end.as_str()
        }
        ScanOperation::ExclusiveRange { from, to } => {
            let start = format!("{}{}", key, from);
            let end = format!("{}{}", key, to);
            k > start.as_str() && k < end.as_str()
        }
        ScanOperation::InclusiveRange { from, to } => {
            let start = format!("{}{}", key, from);
            let end = format!("{}{}", key, to);
            k >= start.as_str() && k <= end.as_str()
        }
        ScanOperation::In { values } => values.iter().any(|v| k.starts_with(v.as_str())),
    }
}

impl KvStore for RedisStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|e| ActError::Store(e.to_string()))?;
        conn.get(key).map_err(|e| ActError::Store(e.to_string()))
    }

    fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|e| ActError::Store(e.to_string()))?;
        conn.set(key, value)
            .map_err(|e| ActError::Store(e.to_string()))
    }

    fn delete(&self, key: &str) -> Result<()> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|e| ActError::Store(e.to_string()))?;
        conn.del(key).map_err(|e| ActError::Store(e.to_string()))
    }

    fn scan_prefix(&self, key: &str, options: ScanOptions) -> Result<Vec<(String, Vec<u8>)>> {
        let ScanOptions {
            is_rev,
            op,
            ref prefix,
        } = options;
        let mut conn = self
            .conn
            .lock()
            .map_err(|e| ActError::Store(e.to_string()))?;
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
                .query(&mut *conn)
                .map_err(|e| ActError::Store(e.to_string()))?;
            for key_str in keys {
                if !key_matches(&key_str, key, prefix, &op) {
                    continue;
                }
                let val: Option<Vec<u8>> = conn
                    .get(&key_str)
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
