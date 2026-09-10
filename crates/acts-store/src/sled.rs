use acts::{ActError, KvStore, Result, ScanOperation, ScanOptions, StoreBatchOp};

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
impl KvStore for SledStore {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.db
            .get(key.as_bytes())
            .map_err(|e| ActError::Store(e.to_string()))
            .map(|opt| opt.map(|ivec| ivec.to_vec()))
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        self.db
            .insert(key.as_bytes(), value)
            .map_err(|e| ActError::Store(e.to_string()))?;
        // Ensure durability — flush to disk
        self.db
            .flush()
            .map(|_| ())
            .map_err(|e| ActError::Store(e.to_string()))
    }

    async fn delete(&self, key: &str) -> Result<()> {
        self.db
            .remove(key.as_bytes())
            .map_err(|e| ActError::Store(e.to_string()))?;
        self.db
            .flush()
            .map(|_| ())
            .map_err(|e| ActError::Store(e.to_string()))
    }

    async fn batch(&self, ops: &[StoreBatchOp]) -> Result<()> {
        if ops.is_empty() {
            return Ok(());
        }
        let mut sled_batch = sled::Batch::default();
        for op in ops {
            match op {
                StoreBatchOp::Put { key, value } => {
                    sled_batch.insert(key.as_bytes(), value.as_slice());
                }
                StoreBatchOp::Delete { key } => {
                    sled_batch.remove(key.as_bytes());
                }
            }
        }
        self.db
            .apply_batch(sled_batch)
            .map_err(|e| ActError::Store(e.to_string()))?;
        // Ensure durability — flush to disk once for the whole batch
        self.db
            .flush()
            .map(|_| ())
            .map_err(|e| ActError::Store(e.to_string()))
    }

    async fn scan_prefix(&self, key: &str, options: ScanOptions) -> Result<Vec<(String, Vec<u8>)>> {
        let ScanOptions {
            is_rev,
            op,
            ref prefix,
        } = options;
        let mut result = Vec::new();
        match &op {
            // A value-key prefix is itself a valid (and much narrower) sled
            // prefix for Eq.
            ScanOperation::Eq if key.starts_with(prefix.as_str()) => {
                for entry in self.db.scan_prefix(key.as_bytes()) {
                    let (k, value) = entry.map_err(|e| ActError::Store(e.to_string()))?;
                    let key_str = String::from_utf8(k.to_vec())
                        .map_err(|e| ActError::Store(e.to_string()))?;
                    result.push((key_str, value.to_vec()));
                }
            }
            ScanOperation::Range { lower, upper } => {
                let start = lower.as_ref().filter(|l| l.starts_with(prefix.as_str()));
                let iterator = match start {
                    Some(lower) => self.db.range(lower.as_bytes()..),
                    None => self.db.scan_prefix(prefix.as_bytes()),
                };
                for entry in iterator {
                    let (k, value) = entry.map_err(|e| ActError::Store(e.to_string()))?;
                    let key_str = String::from_utf8(k.to_vec())
                        .map_err(|e| ActError::Store(e.to_string()))?;
                    if !key_str.starts_with(prefix.as_str()) {
                        break;
                    }
                    if let Some(upper) = upper
                        && key_str.as_str() >= upper.as_str()
                    {
                        break;
                    }
                    if key_matches(&key_str, key, prefix, &op) {
                        result.push((key_str, value.to_vec()));
                    }
                }
            }
            _ => {
                for entry in self.db.scan_prefix(prefix.as_bytes()) {
                    let (k, value) = entry.map_err(|e| ActError::Store(e.to_string()))?;
                    let key_str = String::from_utf8(k.to_vec())
                        .map_err(|e| ActError::Store(e.to_string()))?;
                    if key_matches(&key_str, key, prefix, &op) {
                        result.push((key_str, value.to_vec()));
                    }
                }
            }
        }
        if is_rev {
            result.reverse();
        }
        Ok(result)
    }
}
