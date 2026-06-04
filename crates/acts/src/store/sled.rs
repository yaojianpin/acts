use crate::{ActError, KvStore, Result};

pub struct SledStore {
    db: sled::Db,
}

impl SledStore {
    pub fn open(path: &str) -> Result<Self> {
        let db = sled::open(path).map_err(|e| ActError::Store(e.to_string()))?;
        Ok(Self { db })
    }

    pub fn open_in_memory() -> Result<Self> {
        let db = sled::Config::new()
            .temporary(true)
            .open()
            .map_err(|e| ActError::Store(e.to_string()))?;
        Ok(Self { db })
    }
}

impl KvStore for SledStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.db
            .get(key.as_bytes())
            .map_err(|e| ActError::Store(e.to_string()))
            .map(|opt| opt.map(|ivec| ivec.to_vec()))
    }

    fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        self.db
            .insert(key.as_bytes(), value)
            .map_err(|e| ActError::Store(e.to_string()))?;
        // Ensure durability — flush to disk
        self.db
            .flush()
            .map(|_| ())
            .map_err(|e| ActError::Store(e.to_string()))
    }

    fn delete(&self, key: &str) -> Result<()> {
        self.db
            .remove(key.as_bytes())
            .map_err(|e| ActError::Store(e.to_string()))?;
        self.db
            .flush()
            .map(|_| ())
            .map_err(|e| ActError::Store(e.to_string()))
    }

    fn scan_prefix(&self, prefix: &str, is_rev: bool) -> Result<Vec<(String, Vec<u8>)>> {
        let mut result = Vec::new();
        for entry in self.db.scan_prefix(prefix.as_bytes()) {
            let (key, value) = entry.map_err(|e| ActError::Store(e.to_string()))?;
            let key_str = String::from_utf8(key.to_vec())
                .map_err(|e| ActError::Store(e.to_string()))?;
            result.push((key_str, value.to_vec()));
        }
        if is_rev {
            result.reverse();
        }
        Ok(result)
    }
}
