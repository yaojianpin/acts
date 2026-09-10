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

#[async_trait::async_trait]
impl KvStore for MemoryStore {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.data.read().get(key).cloned())
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        self.data.write().insert(key.to_string(), value);
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<()> {
        self.data.write().remove(key);
        Ok(())
    }

    async fn batch(&self, ops: &[StoreBatchOp]) -> Result<()> {
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

    async fn scan_prefix(&self, key: &str, options: ScanOptions) -> Result<Vec<(String, Vec<u8>)>> {
        let ScanOptions {
            is_rev,
            op,
            ref prefix,
        } = options;
        let map = self.data.read();

        // Start as close as possible to the matching keys. Eq is a full
        // value-key prefix and Range carries an inclusive lower bound, so both
        // provide a BTreeMap range start.
        let start = match &op {
            ScanOperation::Eq if key.starts_with(prefix.as_str()) => key.to_string(),
            ScanOperation::Range {
                lower: Some(lower), ..
            } if lower.starts_with(prefix.as_str()) => lower.clone(),
            _ => prefix.clone(),
        };
        // Guard against invalid direct caller bounds; collection bounds are
        // always ordered.
        let entries = if let ScanOperation::Range {
            lower: Some(lower),
            upper: Some(upper),
        } = &op
            && lower >= upper
        {
            Vec::new()
        } else {
            map.range(start..)
                .take_while(|(k, _)| {
                    if !k.starts_with(prefix.as_str()) {
                        return false;
                    }
                    match &op {
                        ScanOperation::Range {
                            upper: Some(upper), ..
                        } => k.as_str() < upper.as_str(),
                        _ => true,
                    }
                })
                .filter(|(k, _)| key_matches(k, key, prefix, &op))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };
        let mut entries = entries;
        if is_rev {
            entries.reverse();
        }
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn scan(store: &MemoryStore, key: &str, op: ScanOperation, prefix: &str) -> Vec<String> {
        store
            .scan_prefix(key, ScanOptions::new(op, prefix.to_string(), false))
            .await
            .unwrap()
            .into_iter()
            .map(|(key, _)| key)
            .collect()
    }

    #[tokio::test]
    async fn eq_scans_only_the_value_group() {
        let store = MemoryStore::new();
        for id in ["1", "2", "3"] {
            store
                .put(&format!("tasks-pid-a-{id}"), vec![])
                .await
                .unwrap();
        }
        store.put("tasks-pid-b-1", vec![]).await.unwrap();

        let keys = scan(&store, "tasks-pid-a-", ScanOperation::Eq, "tasks-pid-").await;
        assert_eq!(keys, ["tasks-pid-a-1", "tasks-pid-a-2", "tasks-pid-a-3"]);
    }

    #[tokio::test]
    async fn range_starts_at_lower_and_stops_at_upper() {
        let store = MemoryStore::new();
        for id in ["1", "2", "3", "4"] {
            store.put(&format!("tasks-n-{id}"), vec![]).await.unwrap();
        }

        let keys = scan(
            &store,
            "tasks-n-",
            ScanOperation::Range {
                lower: Some("tasks-n-2".to_string()),
                upper: Some("tasks-n-4".to_string()),
            },
            "tasks-n-",
        )
        .await;
        assert_eq!(keys, ["tasks-n-2", "tasks-n-3"]);
    }
}
