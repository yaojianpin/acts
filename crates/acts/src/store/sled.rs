use crate::store::{ScanOperation, ScanOptions};
use crate::{ActError, KvStore, Result};

pub struct SledStore {
    db: sled::Db,
}

impl SledStore {
    pub fn open(path: &str) -> Result<Self> {
        let db = sled::open(path).map_err(|e| ActError::Store(e.to_string()))?;
        Ok(Self { db })
    }

    #[allow(dead_code)]
    pub fn open_in_memory() -> Result<Self> {
        let db = sled::Config::new()
            .temporary(true)
            .open()
            .map_err(|e| ActError::Store(e.to_string()))?;
        Ok(Self { db })
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

    fn scan_prefix(&self, key: &str, options: ScanOptions) -> Result<Vec<(String, Vec<u8>)>> {
        let ScanOptions {
            is_rev,
            op,
            ref prefix,
        } = options;
        // Sled's scan_prefix returns keys in ascending order, bounded by prefix
        let mut result = Vec::new();
        for entry in self.db.scan_prefix(prefix.as_bytes()) {
            let (k, value) = entry.map_err(|e| ActError::Store(e.to_string()))?;
            let key_str =
                String::from_utf8(k.to_vec()).map_err(|e| ActError::Store(e.to_string()))?;
            if key_matches(&key_str, key, prefix, &op) {
                result.push((key_str, value.to_vec()));
            }
        }
        if is_rev {
            result.reverse();
        }
        Ok(result)
    }
}
