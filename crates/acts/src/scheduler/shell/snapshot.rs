//! The registry of snapshot-backed sealed-data targets (see
//! [`crate::snapshot`]): the in-memory cache the snapshot TTL sweep purges.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

use crate::ShareLock;
use crate::snapshot::{SnapshotOptions, SnapshotStore};

/// Registry of snapshot-backed sealed-data targets (see [`crate::snapshot`]).
pub(crate) struct SnapshotRegistry {
    stores: ShareLock<HashMap<String, Arc<SnapshotStore>>>,
}

impl SnapshotRegistry {
    pub(crate) fn new() -> Self {
        Self {
            stores: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.stores.read().len()
    }

    /// Register (or replace) a snapshot target. Replacing a name drops its
    /// cached values.
    pub(crate) fn register(&self, name: &str, options: SnapshotOptions) -> Arc<SnapshotStore> {
        let store = Arc::new(SnapshotStore::new(options));
        self.stores.write().insert(name.to_string(), store.clone());
        store
    }

    pub(crate) fn store(&self, name: &str) -> Option<Arc<SnapshotStore>> {
        self.stores.read().get(name).cloned()
    }

    pub(crate) fn list(&self) -> Vec<(String, Arc<SnapshotStore>)> {
        self.stores
            .read()
            .iter()
            .map(|(name, store)| (name.clone(), store.clone()))
            .collect()
    }

    /// Drop expired entries of every registered store (TTL sweep).
    pub(crate) fn purge_expired(&self) -> usize {
        self.list()
            .into_iter()
            .map(|(_, store)| store.purge_expired())
            .sum()
    }
}
