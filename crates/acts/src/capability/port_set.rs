//! A typed, keyed set of port implementations.

use crate::{ActError, Result};
use std::{collections::BTreeMap, sync::Arc};

/// A set of implementations of one port trait, keyed by an external string.
///
/// The key is the provider key a plugin chooses (`acme.participant`); the
/// kernel only defines the capability type (`T`, for example
/// `dyn ParticipantResolver`). There is no `Any` in the map and no
/// downcasting: a `PortSet<T>` only ever yields `Arc<T>`.
///
/// A set may also name one registered key as its default, so an unconfigured
/// caller can use the only (or preferred) implementation. Registration and
/// default selection reject an empty key, a duplicate key and a default that
/// names no registered key. The set is assembled during the startup pass and
/// read-only afterwards; it does not own that lifecycle itself.
pub struct PortSet<T: ?Sized + Send + Sync> {
    entries: BTreeMap<String, Arc<T>>,
    default: Option<String>,
}

impl<T: ?Sized + Send + Sync> PortSet<T> {
    /// An empty set with no default.
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            default: None,
        }
    }

    /// Register `port` under `key`.
    ///
    /// The key must be non-empty and unused; registration never replaces an
    /// existing implementation.
    pub fn register(&mut self, key: impl Into<String>, port: Arc<T>) -> Result<()> {
        let key = key.into();
        if key.is_empty() {
            return Err(ActError::Config("port key '' is empty".to_string()));
        }
        if self.entries.contains_key(&key) {
            return Err(ActError::Config(format!(
                "port key '{key}' is already registered"
            )));
        }
        self.entries.insert(key, port);
        Ok(())
    }

    /// The implementation registered under `key`, if any.
    pub fn get(&self, key: &str) -> Option<Arc<T>> {
        self.entries.get(key).cloned()
    }

    /// Replace the set with one default implementation under `key`.
    ///
    /// This is the kernel's internal assembly path for a built-in default, not
    /// a provider registration: it discards any previous implementation (the
    /// serving host replaces the shadow transport), and the key is an internal
    /// constant, never a plugin's key. A plugin's registration goes through
    /// [`Self::register`] and keeps the duplicate-key refusal.
    pub(crate) fn install_default(&mut self, key: &str, port: Arc<T>) {
        assert!(!key.is_empty(), "a port key must not be empty");
        self.entries.clear();
        self.entries.insert(key.to_string(), port);
        self.default = Some(key.to_string());
    }

    /// The implementation named by [`Self::set_default`], if a default was set.
    pub fn default_port(&self) -> Option<Arc<T>> {
        self.default.as_deref().and_then(|key| self.get(key))
    }

    /// Make the registered `key` the set's default.
    ///
    /// Setting a default that names no registered key is refused, so
    /// [`Self::default_port`] never returns `None` for a configured default.
    pub fn set_default(&mut self, key: &str) -> Result<()> {
        if !self.entries.contains_key(key) {
            return Err(ActError::Config(format!(
                "cannot set default port: key '{key}' is not registered"
            )));
        }
        self.default = Some(key.to_string());
        Ok(())
    }

    /// Whether `key` is registered.
    pub fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    /// The registered keys in lexicographic order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// The number of registered implementations.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no implementation is registered.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl<T: ?Sized + Send + Sync> Default for PortSet<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    trait TestPort: Send + Sync {
        fn id(&self) -> u32;
    }

    struct Fixed(u32);

    impl TestPort for Fixed {
        fn id(&self) -> u32 {
            self.0
        }
    }

    fn port(id: u32) -> Arc<dyn TestPort> {
        Arc::new(Fixed(id))
    }

    #[test]
    fn register_and_get_by_key() {
        let mut ports: PortSet<dyn TestPort> = PortSet::new();
        ports.register("alpha", port(1)).unwrap();
        ports.register("beta", port(2)).unwrap();

        assert_eq!(ports.get("alpha").unwrap().id(), 1);
        assert_eq!(ports.get("beta").unwrap().id(), 2);
        assert!(ports.get("missing").is_none());
        assert!(ports.contains("alpha"));
        assert!(!ports.contains("missing"));
    }

    #[test]
    fn default_port_is_selected_explicitly() {
        let mut ports: PortSet<dyn TestPort> = PortSet::new();
        assert!(ports.default_port().is_none());

        ports.register("alpha", port(1)).unwrap();
        ports.register("beta", port(2)).unwrap();
        ports.set_default("beta").unwrap();
        assert_eq!(ports.default_port().unwrap().id(), 2);

        ports.set_default("alpha").unwrap();
        assert_eq!(ports.default_port().unwrap().id(), 1);
    }

    #[test]
    fn duplicate_key_is_refused() {
        let mut ports: PortSet<dyn TestPort> = PortSet::new();
        ports.register("alpha", port(1)).unwrap();

        let err = ports.register("alpha", port(2)).unwrap_err();
        assert!(matches!(err, ActError::Config(_)));
        assert!(err.to_string().contains("alpha"));
        assert_eq!(
            ports.get("alpha").unwrap().id(),
            1,
            "a refused duplicate must keep the first registration"
        );
    }

    #[test]
    fn empty_key_is_refused() {
        let mut ports: PortSet<dyn TestPort> = PortSet::new();

        let err = ports.register("", port(1)).unwrap_err();
        assert!(matches!(err, ActError::Config(_)));
        assert!(err.to_string().contains("''"));
        assert!(ports.is_empty());
    }

    #[test]
    fn unknown_default_is_refused() {
        let mut ports: PortSet<dyn TestPort> = PortSet::new();

        let err = ports.set_default("missing").unwrap_err();
        assert!(matches!(err, ActError::Config(_)));
        assert!(err.to_string().contains("missing"));
        assert!(ports.default_port().is_none());
    }

    #[test]
    fn keys_len_and_is_empty() {
        let mut ports: PortSet<dyn TestPort> = PortSet::new();
        assert!(ports.is_empty());
        assert_eq!(ports.len(), 0);

        ports.register("beta", port(2)).unwrap();
        ports.register("alpha", port(1)).unwrap();
        assert!(!ports.is_empty());
        assert_eq!(ports.len(), 2);
        assert_eq!(
            ports.keys().collect::<Vec<_>>(),
            vec!["alpha", "beta"],
            "keys are lexicographic"
        );
    }
}
