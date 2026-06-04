use crate::{ActError, KvStore, Result};
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

    fn scan_prefix(&self, prefix: &str, is_rev: bool) -> Result<Vec<(String, Vec<u8>)>> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|e| ActError::Store(e.to_string()))?;
        let pattern = format!("{}*", prefix);
        let keys: Vec<String> = conn
            .keys(&pattern)
            .map_err(|e| ActError::Store(e.to_string()))?;
        let mut result = Vec::with_capacity(keys.len());
        for key in keys {
            let val: Option<Vec<u8>> =
                conn.get(&key).map_err(|e| ActError::Store(e.to_string()))?;
            if let Some(v) = val {
                result.push((key, v));
            }
        }
        if is_rev {
            result.reverse();
        }
        Ok(result)
    }
}
