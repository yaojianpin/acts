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
    /// The publishing package id of each variable-registered target — the
    /// provider identity a same-name registration distinguishes by.
    providers: ShareLock<HashMap<String, Vec<String>>>,
}

impl SnapshotRegistry {
    pub(crate) fn new() -> Self {
        Self {
            stores: Arc::new(RwLock::new(HashMap::new())),
            providers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.stores.read().len()
    }

    /// Register a variable-provided target under one write lock. `provider`
    /// is optional — `None` is the **global** registration (every run reads
    /// it unless its context names a provider). The `(name, provider)` pair
    /// is the conflict key:
    ///
    /// - the same name and the same provider twice is refused: a provider
    ///   registers a variable once, and a changed registration means a new
    ///   package version (or a new provider id), never a silent replacement;
    /// - the same name from different providers (global included) is several
    ///   registrations — the pull parameters must match (the first publisher
    ///   fixes them) and each provider id joins the provider list. Runs whose
    ///   sealed context names a provider are served by it; the rest fall back
    ///   to the global registration.
    pub(crate) fn register_snap(
        &self,
        name: &str,
        options: SnapshotOptions,
        provider: Option<&str>,
    ) -> std::result::Result<Arc<SnapshotStore>, String> {
        let provider = provider.unwrap_or_default().trim().to_string();
        let shown = if provider.is_empty() {
            "global".to_string()
        } else {
            provider.clone()
        };
        let mut stores = self.stores.write();
        let mut providers = self.providers.write();
        let list = providers.entry(name.to_string()).or_default();
        if list.iter().any(|registered| registered == &provider) {
            return Err(format!(
                "snapshot variable '{name}' is already registered by \
                 provider '{shown}'; a provider registers a variable once"
            ));
        }
        if let Some(existing) = stores.get(name) {
            if existing.options != options {
                return Err(format!(
                    "snapshot variable '{name}' is already registered with \
                     different parameters; the first publisher wins"
                ));
            }
            list.push(provider);
            return Ok(existing.clone());
        }
        let store = Arc::new(SnapshotStore::new(options));
        stores.insert(name.to_string(), store.clone());
        list.push(provider);
        Ok(store)
    }

    /// The packages registered as providers of `name` (a variable), in
    /// registration order.
    pub(crate) fn providers(&self, name: &str) -> Vec<String> {
        self.providers.read().get(name).cloned().unwrap_or_default()
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
