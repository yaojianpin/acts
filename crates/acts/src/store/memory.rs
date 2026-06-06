use crate::Result;
use dashmap::DashMap;

use crate::store::{KvStore, ScanOperation, ScanOptions};

#[derive(Debug)]
pub struct MemoryStore {
    data: DashMap<String, Vec<u8>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self {
            data: DashMap::new(),
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

impl KvStore for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.data.get(key).map(|v| v.clone()))
    }

    fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        self.data.insert(key.to_string(), value);
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<()> {
        self.data.remove(key);
        Ok(())
    }

    fn scan_prefix(&self, key: &str, options: ScanOptions) -> Result<Vec<(String, Vec<u8>)>> {
        let ScanOptions {
            is_rev,
            op,
            ref prefix,
        } = options;
        let mut entries: Vec<(String, Vec<u8>)> = self
            .data
            .iter()
            .filter(|entry| key_matches(entry.key(), key, prefix, &op))
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect();
        if is_rev {
            entries.reverse();
        }
        Ok(entries)
    }
}
