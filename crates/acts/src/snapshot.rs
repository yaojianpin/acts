//! Snapshot-backed sealed data.
//!
//! External systems feed versioned key-value snapshots into the engine
//! through [`SnapshotManager`] (the write path: a gRPC/NATS/Kafka adapter,
//! or an embedder calling `engine.snapshot()` directly). At each task's
//! prepare the scheduler reads the local snapshot for the task's scope and
//! seals it — `resolve` never performs network I/O, so remote latency and
//! outages stay out of the scheduling hot path.
//!
//! Two policies control *when* the cache value is frozen into a task's
//! sealed data:
//!
//! - [`SnapshotPolicy::PerProc`] (default): seal once per task lineage — the
//!   first task whose ancestor chain has no sealed value resolves, later
//!   tasks inherit the pinned value. A retried task keeps its first value.
//!   Use when the scope is fixed for the whole process (tenant, env, …).
//! - [`SnapshotPolicy::PerTask`]: every task re-reads the cache at its own
//!   prepare, so each new task sees the latest value. A retried task still
//!   keeps the value it first sealed (determinism across replay). Use when
//!   the scope varies inside one process (project/unit per step) or a step
//!   must observe recent external changes.
//!
//! Snapshots are in-memory only. Per-process pinned values are durable via
//! the task's sealed vars row; durability of the snapshot itself comes from
//! the message bus (compacted topic / JetStream / watch) that rebuilds this
//! cache after a restart.
//!
//! Caches stay bounded by configuration: feeds should `remove()` a scope
//! when the source deletes it (tombstone), and [`SnapshotOptions::ttl_secs`]
//! drops entries that were not refreshed in time — lazily on read and by a
//! periodic sweep — so a forgotten scope cannot grow the cache forever.

use crate::{ActError, Vars, config::MissingParamAction, scheduler::Runtime};
use parking_lot::RwLock;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

/// When a snapshot value is frozen into the task's sealed data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SnapshotPolicy {
    /// Seal once per task lineage (first resolver run); descendants inherit
    /// the pinned value — frozen for the whole process.
    #[default]
    PerProc,
    /// Every task reads the latest cache value at its own prepare.
    PerTask,
}

/// Registration options of a snapshot-backed sealed-data target.
#[derive(Debug, Clone)]
pub struct SnapshotOptions {
    pub policy: SnapshotPolicy,
    /// Task parameters (found via parent-chain traversal) joined with `/`
    /// into the snapshot scope key. Empty: one global scope per target.
    pub key_params: Vec<String>,
    /// What happens when a key param or the snapshot data is absent.
    pub on_missing: MissingParamAction,
    /// Seconds an entry stays valid after its last refresh; `None` never
    /// expires. Expired entries are dropped on read and by the periodic
    /// sweep — with `remove()` tombstones the backstop against unbounded
    /// growth of dead scopes.
    pub ttl_secs: Option<u64>,
}

impl Default for SnapshotOptions {
    fn default() -> Self {
        Self {
            policy: SnapshotPolicy::PerProc,
            key_params: Vec::new(),
            on_missing: MissingParamAction::Skip,
            ttl_secs: None,
        }
    }
}

impl SnapshotOptions {
    pub fn per_proc() -> Self {
        Self::default()
    }
    pub fn per_task() -> Self {
        Self {
            policy: SnapshotPolicy::PerTask,
            ..Default::default()
        }
    }

    pub fn with_ttl(mut self, ttl_secs: u64) -> Self {
        self.ttl_secs = Some(ttl_secs);
        self
    }
}

/// One versioned snapshot value for a scope key.
#[derive(Debug, Clone)]
pub struct SnapshotEntry {
    /// Monotonic revision supplied by the bus (offset/seq) or the producer.
    pub rev: u64,
    pub data: Vars,
    /// Receive timestamp in milliseconds since the unix epoch — the expiry
    /// basis when `ttl_secs` is set.
    pub timestamp: i64,
}

/// The in-memory snapshot cache of one sealed-data target.
pub(crate) struct SnapshotStore {
    pub(crate) options: SnapshotOptions,
    entries: RwLock<HashMap<String, SnapshotEntry>>,
}

impl SnapshotStore {
    pub(crate) fn new(options: SnapshotOptions) -> Self {
        Self {
            options,
            entries: RwLock::new(HashMap::new()),
        }
    }

    pub(crate) fn get(&self, scope: &str) -> Option<SnapshotEntry> {
        let entry = self.entries.read().get(scope).cloned();
        if let Some(ttl) = self.options.ttl_secs
            && let Some(e) = &entry
            && now_ms() - e.timestamp > ttl as i64 * 1000
        {
            // expired: drop on the read path (a concurrent refresh re-adds)
            self.entries.write().remove(scope);
            return None;
        }
        entry
    }

    pub(crate) fn upsert(&self, scope: &str, rev: u64, data: Vars) {
        self.entries.write().insert(
            scope.to_string(),
            SnapshotEntry {
                rev,
                data,
                timestamp: now_ms(),
            },
        );
    }

    /// Remove the snapshot for a scope (tombstone).
    pub(crate) fn remove(&self, scope: &str) {
        self.entries.write().remove(scope);
    }

    /// Drop every entry whose ttl elapsed; returns the number removed.
    /// No-op when the target has no ttl.
    pub(crate) fn purge_expired(&self) -> usize {
        let Some(ttl) = self.options.ttl_secs else {
            return 0;
        };
        let now = now_ms();
        let mut guard = self.entries.write();
        let before = guard.len();
        guard.retain(|_, e| now - e.timestamp <= ttl as i64 * 1000);
        before - guard.len()
    }

    /// All entries of the store: `(scope, entry)` pairs.
    pub(crate) fn list(&self) -> Vec<(String, SnapshotEntry)> {
        self.entries
            .read()
            .iter()
            .map(|(scope, entry)| (scope.clone(), entry.clone()))
            .collect()
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Write/read handle of the snapshot caches, obtained via [`Engine::snapshot`](crate::Engine::snapshot).
///
/// Feed adapters (message channels) call [`upsert`](Self::upsert) /
/// [`remove`](Self::remove) when data arrives; the scheduler reads the same
/// store at each task prepare. `upsert` auto-registers the target with
/// default options when it does not exist yet, so a feed never drops data on
/// an unregistered name — register explicitly first when a non-default
/// policy or scope keys are needed.
#[derive(Clone)]
pub struct SnapshotManager {
    runtime: Arc<Runtime>,
}

impl SnapshotManager {
    pub(crate) fn new(runtime: &Arc<Runtime>) -> Self {
        Self {
            runtime: runtime.clone(),
        }
    }

    /// Register a snapshot target with its options (replaces any existing
    /// registration of the same name — its cache is reset).
    pub fn register(&self, name: &str, options: SnapshotOptions) {
        self.runtime.register_snapshot(name, options);
    }

    /// Feed a new value for `name`/`scope`. Auto-registers the target with
    /// [`SnapshotOptions::default`] when missing.
    pub fn upsert(&self, name: &str, scope: &str, rev: u64, data: Vars) {
        let store = self.runtime.snapshot_store(name).unwrap_or_else(|| {
            self.runtime
                .register_snapshot(name, SnapshotOptions::default())
        });
        store.upsert(scope, rev, data);
    }

    /// Remove the value of `name`/`scope` (tombstone).
    pub fn remove(&self, name: &str, scope: &str) {
        if let Some(store) = self.runtime.snapshot_store(name) {
            store.remove(scope);
        }
    }

    /// Current value of `name`/`scope`, if any.
    pub fn read(&self, name: &str, scope: &str) -> Option<SnapshotEntry> {
        self.runtime.snapshot_store(name)?.get(scope)
    }
    /// All current values of one snapshot target: `(scope, entry)` pairs.
    pub fn list(&self, name: &str) -> Vec<(String, SnapshotEntry)> {
        match self.runtime.snapshot_store(name) {
            Some(store) => store.list(),
            None => Vec::new(),
        }
    }
}

#[allow(dead_code)]
fn missing_err(name: &str, missing: &[String]) -> crate::ActError {
    ActError::Runtime(format!(
        "snapshot '{name}' missing required params: {missing:?}"
    ))
}

/// Canonical scope-key fragment: strings without quotes, other json verbatim.
fn scope_fragment(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Join the given task params (already resolved by the caller) into a scope key.
pub(crate) fn join_scope(values: &[serde_json::Value]) -> String {
    let mut scope = String::new();
    for v in values {
        if !scope.is_empty() {
            scope.push('/');
        }
        scope.push_str(&scope_fragment(v));
    }
    scope
}

/// Validate that every key param of `options` resolves on the task chain,
/// returning the raw values in order, or the missing names.
pub(crate) fn resolve_scope_params(
    task: &crate::scheduler::Task,
    options: &SnapshotOptions,
) -> std::result::Result<Vec<serde_json::Value>, Vec<String>> {
    let mut values = Vec::with_capacity(options.key_params.len());
    let mut missing = Vec::new();
    for p in &options.key_params {
        match task.find::<serde_json::Value>(p) {
            Some(v) => values.push(v),
            None => missing.push(p.clone()),
        }
    }
    if missing.is_empty() {
        Ok(values)
    } else {
        Err(missing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_scope_strings_and_numbers() {
        assert_eq!(join_scope(&[]), "");
        assert_eq!(join_scope(&[serde_json::json!("u1")]), "u1");
        assert_eq!(
            join_scope(&[serde_json::json!("u1"), serde_json::json!("proj-a")]),
            "u1/proj-a"
        );
        assert_eq!(join_scope(&[serde_json::json!(7)]), "7");
    }

    #[test]
    fn store_upsert_read_remove() {
        let store = SnapshotStore::new(SnapshotOptions::default());
        assert!(store.get("s1").is_none());
        store.upsert("s1", 1, Vars::new().with("a", 1));
        let entry = store.get("s1").unwrap();
        assert_eq!(entry.rev, 1);
        assert_eq!(entry.data.get::<i32>("a").unwrap(), 1);
        assert!(entry.timestamp > 0);

        // newer revision replaces
        store.upsert("s1", 2, Vars::new().with("a", 2));
        assert_eq!(store.get("s1").unwrap().rev, 2);

        // different scopes are independent
        assert!(store.get("s2").is_none());

        // tombstone removes
        store.remove("s1");
        assert!(store.get("s1").is_none());
    }

    #[test]
    fn missing_err_format() {
        let err = missing_err("profile", &["unit".to_string(), "project".to_string()]);
        assert!(
            err.to_string().contains("profile") && err.to_string().contains("unit"),
            "got: {err}"
        );
    }

    #[test]
    fn store_ttl_expires_on_read_and_purges() {
        // fresh entry: readable
        let store = SnapshotStore::new(SnapshotOptions::default().with_ttl(1));
        store.upsert("s1", 1, Vars::new().with("a", 1));
        assert!(store.get("s1").is_some());

        // no ttl configured: entries never expire
        let forever = SnapshotStore::new(SnapshotOptions::default());
        forever.upsert("keep", 1, Vars::new().with("a", 1));

        std::thread::sleep(std::time::Duration::from_millis(1100));

        // expired: the read path drops it
        assert!(store.get("s1").is_none());
        assert!(forever.get("keep").is_some());

        // refresh restores visibility
        store.upsert("s1", 2, Vars::new().with("a", 2));
        assert!(store.get("s1").is_some());

        // purge removes only the expired ones
        let multi = SnapshotStore::new(SnapshotOptions::default().with_ttl(1));
        multi.upsert("old", 1, Vars::new().with("a", 1));
        std::thread::sleep(std::time::Duration::from_millis(1100));
        multi.upsert("new", 2, Vars::new().with("a", 2));
        assert_eq!(multi.purge_expired(), 1);
        assert!(multi.get("old").is_none());
        assert!(multi.get("new").is_some());
        assert_eq!(multi.purge_expired(), 0);
    }
}
