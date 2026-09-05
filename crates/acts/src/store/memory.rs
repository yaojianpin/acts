use crate::Result;
use parking_lot::RwLock;
use std::collections::BTreeMap;

use crate::store::{KvStore, ScanOperation, ScanOptions, StoreBatchOp};

#[derive(Debug)]
pub struct MemoryStore {
    data: RwLock<BTreeMap<String, Vec<u8>>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self {
            data: RwLock::new(BTreeMap::new()),
        }
    }
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
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

impl KvStore for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.data.read().get(key).cloned())
    }

    fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        self.data.write().insert(key.to_string(), value);
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<()> {
        self.data.write().remove(key);
        Ok(())
    }

    fn batch(&self, ops: &[StoreBatchOp]) -> Result<()> {
        if ops.is_empty() {
            return Ok(());
        }
        // One write lock for the whole batch: concurrent readers can never
        // observe a partially applied batch.
        let mut data = self.data.write();
        for op in ops {
            match op {
                StoreBatchOp::Put { key, value } => {
                    data.insert(key.clone(), value.clone());
                }
                StoreBatchOp::Delete { key } => {
                    data.remove(key.as_str());
                }
            }
        }
        Ok(())
    }

    fn scan_prefix(&self, key: &str, options: ScanOptions) -> Result<Vec<(String, Vec<u8>)>> {
        let ScanOptions {
            is_rev,
            op,
            ref prefix,
        } = options;
        let map = self.data.read();
        let mut entries: Vec<(String, Vec<u8>)> = map
            .range(prefix.clone()..)
            .take_while(|(k, _)| k.starts_with(prefix.as_str()))
            .filter(|(k, _)| key_matches(k, key, prefix, &op))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if is_rev {
            entries.reverse();
        }
        Ok(entries)
    }
}
