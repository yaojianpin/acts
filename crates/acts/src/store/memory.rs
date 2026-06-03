use crate::Result;
use dashmap::DashMap;

use crate::store::KvStore;

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

    fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>> {
        Ok(self
            .data
            .iter()
            .filter(|entry| entry.key().starts_with(prefix))
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect())
    }
}
