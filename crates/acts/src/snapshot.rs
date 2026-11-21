//! Snapshot-backed sealed data.
//!
//! External systems feed versioned key-value snapshots into the engine
//! through [`SnapshotManager`] (the write path: a gRPC/NATS/Kafka adapter,
//! or an embedder calling `engine.snapshot()` directly). At each effect
//! prepare the scheduler reads the local snapshot for the acting
//! execution's scope and seals it — the *seal itself* is a local cache read,
//! so expression evaluation never performs network I/O; remote latency and
//! outages stay out of the evaluation path.
//!
//! Two data paths meet at that cache:
//!
//! - **push** (the default): feeds write out-of-band; the seal reads what is
//!   there and a miss follows the target's `on_missing`.
//! - **pull** (a *snapshot variable*, registered by a provider package's
//!   `pack:publish` metadata — `options.var`): on a miss the seal asks the
//!   variable's provider once — a `type: "req"` message to the matching
//!   channels, one bounded wait (`FetchOptions::timeout_ms`, clamped to
//!   `[100, 30_000]`) — and the provider answers through `snap:reply`, which
//!   lands the value under the same revision guard as a push feed. The
//!   request carries only names (target, scope, the run's pid, the sealed
//!   owner context); the engine's req gate withholds it from channels whose
//!   subject may not read the scope it names (see
//!   [`crate::acl::AccessControl::principal_of`]), and only the first
//!   prepare per `(target, scope)` calls out — the rest join its wait. A
//!   timed-out fetch folds into `on_missing`, and a plain feed's
//!   auto-registration can never open the pull path.
//!
//! The scope key of a fetch target without declared `scope` params is the
//! sealed owner context, sorted and `/`-joined — the same keys the provider
//! filters its subscription on.
//!
//! Feeds stamp a monotonic `rev` per scope. `upsert` applies a value only
//! when its revision is newer than the cached one, so a delayed retry, an
//! out-of-order bus delivery, or a race between two feeds can never roll a
//! scope back to an older value.
//!
//! Two policies keep the legacy vocabulary for *when* the cache value is
//! resolved. In the kernel both resolve at each effect prepare: each effect
//! builds a fresh headless scope, so a value is never carried from one
//! prepare to the next (see [`SnapshotPolicy`]).
//!
//! ## Sealed values are per effect, not durable run state
//!
//! Sealing is the effect seam's authorization and data-resolution step, not
//! a recoverable part of a run. The value lives only in the headless
//! `ScopeTask` of that evaluation:
//!
//! - The aggregate (`ProcessSnapshot`) carries no sealed area, and the
//!   projected `TaskVars` row keeps its fixed shape with an empty `sealed`
//!   field. `proc:get` has no sealed column and answers only the task's
//!   `data`.
//! - A restarted or resumed run does not rebuild the sealed values an
//!   earlier effect saw. The next effect resolves from the snapshot cache as
//!   rebuilt by the feeds; outputs and params an effect already committed
//!   are in the aggregate and are never sealed again.
//! - Branch predicates evaluate without `Context::prepare`, so a
//!   `$target.key` read there resolves to nothing; only the effect seam
//!   seals.
//!
//! Snapshots are in-memory only. Durability of the cache comes from the
//! message bus (compacted topic / JetStream / watch) that rebuilds it after
//! a restart — never from the sealed vars row.
//!
//! Caches stay bounded by configuration: feeds should `remove()` a scope
//! when the source deletes it (tombstone), and [`SnapshotOptions::ttl_secs`]
//! drops entries that were not refreshed in time — lazily on read and by a
//! periodic sweep — so a forgotten scope cannot grow the cache forever. The
//! read-path drop is revision-guarded: it only removes the exact expired entry
//! the reader observed, never a refresh that landed while it was reading.

use crate::{
    ActError, ActRunAs, ShareLock, Vars,
    config::MissingParamAction,
    event::{Message, MessageState},
    scheduler::Runtime,
    utils::time,
};
use parking_lot::RwLock;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

/// When a snapshot value is resolved into the preparing scope.
///
/// The kernel's effect seam prepares a fresh headless scope per effect, so
/// both variants are observably "resolve once at this prepare": a value is
/// not pinned across effects, retries or a restart. The names remain for
/// configuration compatibility; see the [module docs](self) for why sealed
/// data is not durable run state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotPolicy {
    /// The legacy "seal once per task lineage" policy. A headless effect
    /// scope has no durable lineage, so it resolves once for that effect's
    /// prepare — the same observable rule as [`SnapshotPolicy::PerTask`].
    #[default]
    PerProc,
    /// Resolve the latest cache value at this prepare (one per effect).
    PerTask,
}

/// Largest ttl representable as an expiry deadline: the entry timestamp plus
/// `ttl * 1000` must stay within the `i64` epoch-millisecond range of
/// [`SnapshotEntry::timestamp`]. Larger values are rejected by
/// [`SnapshotOptions::validate`]; a store built directly from such options
/// saturates instead so it can never overflow or panic.
pub const MAX_TTL_SECS: u64 = i64::MAX as u64 / 1000;

/// Registration options of a snapshot-backed sealed-data target.
#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotOptions {
    /// When a value is resolved. Both variants resolve at each effect
    /// prepare in the kernel; see [`SnapshotPolicy`].
    pub policy: SnapshotPolicy,
    /// Task param names whose values join with `/` into this target's scope
    /// key. Empty: one global scope per target — except on a fetch target,
    /// where the sealed owner context composes the scope key instead (see
    /// [`Self::fetch`]).
    pub scope: Vec<String>,
    /// What happens when a scope param or the snapshot data is absent.
    pub on_missing: MissingParamAction,
    /// Seconds an entry stays valid after its last refresh; `None` never
    /// expires. Expired entries are dropped on read and by the periodic
    /// sweep — with `remove()` tombstones the backstop against unbounded
    /// growth of dead scopes.
    ///
    /// `Some(0)` is a zero-length window: the entry is expired the moment it
    /// is read. Values above [`MAX_TTL_SECS`] cannot be represented as a
    /// deadline and are rejected by [`SnapshotOptions::validate`].
    pub ttl_secs: Option<u64>,
    /// The pull path. `Some`: on a cache miss the seal may ask the target's
    /// provider once (a `req` message to the matching channels, one bounded
    /// wait, the reply lands as a `snap:reply` — the same rev guard as a
    /// push feed). `None`: pure push, the historical behavior. Only a
    /// variable registration (`pack:publish` metadata) opens this — a plain
    /// feed auto-registration can never turn a target into a caller.
    pub fetch: Option<FetchOptions>,
}

/// The pull parameters of a fetch-enabled snapshot target.
#[derive(Debug, Clone, PartialEq)]
pub struct FetchOptions {
    /// Single-fetch upper bound in milliseconds; clamped to `[100, 30_000]`
    /// by [`Self::timeout`] so a misconfiguration can neither spin the seal
    /// nor hold an effect on a dead provider for half a minute.
    pub timeout_ms: u64,
}

impl Default for FetchOptions {
    fn default() -> Self {
        Self { timeout_ms: 3000 }
    }
}

impl FetchOptions {
    /// The clamped wait. Below 100 ms no remote round trip completes
    /// reliably; above 30 s an outage would hold every cold act of the run.
    pub fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.timeout_ms.clamp(100, 30_000))
    }
}

impl Default for SnapshotOptions {
    fn default() -> Self {
        Self {
            policy: SnapshotPolicy::PerProc,
            scope: Vec::new(),
            on_missing: MissingParamAction::Skip,
            ttl_secs: None,
            fetch: None,
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

    /// Set the ttl in seconds; see [`SnapshotOptions::ttl_secs`] for the
    /// accepted range and the zero semantics.
    pub fn with_ttl(mut self, ttl_secs: u64) -> Self {
        self.ttl_secs = Some(ttl_secs);
        self
    }

    /// Reject a ttl that cannot be represented as an expiry deadline:
    /// [`MAX_TTL_SECS`] is the largest valid value. A zero ttl is valid and
    /// means entries are never readable.
    pub fn validate(&self) -> crate::Result<()> {
        match self.ttl_secs {
            Some(ttl) if ttl > MAX_TTL_SECS => Err(ActError::Config(format!(
                "snapshot ttl {ttl}s exceeds the maximum of {MAX_TTL_SECS}s"
            ))),
            _ => Ok(()),
        }
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
    /// ttl in milliseconds, precomputed once at construction. Saturating so a
    /// store built from an out-of-range [`SnapshotOptions::ttl_secs`] — the
    /// field is public, so direct construction bypasses
    /// [`SnapshotOptions::validate`] — can never overflow.
    ttl_ms: Option<u64>,
    entries: RwLock<HashMap<String, SnapshotEntry>>,
}

impl SnapshotStore {
    pub(crate) fn new(options: SnapshotOptions) -> Self {
        let ttl_ms = options.ttl_secs.map(|ttl| ttl.saturating_mul(1000));
        Self {
            options,
            ttl_ms,
            entries: RwLock::new(HashMap::new()),
        }
    }

    pub(crate) fn get(&self, scope: &str) -> Option<SnapshotEntry> {
        let observed = self.entries.read().get(scope).cloned()?;
        if !self.is_expired(&observed) {
            return Some(observed);
        }
        self.evict_expired(scope, &observed)
    }

    /// Whether `entry` reached its ttl. See [`is_expired_at`] for the
    /// boundary semantics.
    fn is_expired(&self, entry: &SnapshotEntry) -> bool {
        self.ttl_ms
            .is_some_and(|ttl_ms| is_expired_at(ttl_ms, entry.timestamp, now_ms()))
    }

    /// Drop the expired entry a reader observed, but only while it is still
    /// that exact entry — same `rev` and `timestamp`. A concurrent refresh
    /// (`upsert`) replaces it, and that value must survive the eviction: when
    /// the cached entry differs from the observed one, it is returned to the
    /// reader instead of being dropped. `None` means nothing to read — the
    /// entry was evicted now, concurrently removed, or replaced by one that
    /// is expired as well (left for the purge sweep).
    fn evict_expired(&self, scope: &str, observed: &SnapshotEntry) -> Option<SnapshotEntry> {
        let mut entries = self.entries.write();
        let cur = entries.get(scope)?;
        if cur.rev != observed.rev || cur.timestamp != observed.timestamp {
            return (!self.is_expired(cur)).then(|| cur.clone());
        }
        entries.remove(scope);
        None
    }

    /// Insert or replace the value of `scope`.
    ///
    /// Only a strictly newer `rev` overwrites the cached entry; a stale
    /// revision (older than the cached one) is dropped, and an equal revision
    /// is treated as an idempotent replay — the cached value is kept and only
    /// the ttl basis is refreshed.
    pub(crate) fn upsert(&self, scope: &str, rev: u64, data: Vars) {
        let now = now_ms();
        let mut entries = self.entries.write();
        if let Some(entry) = entries.get_mut(scope) {
            if rev > entry.rev {
                entry.rev = rev;
                entry.data = data;
                entry.timestamp = now;
            } else if rev == entry.rev {
                entry.timestamp = now;
            }
            return;
        }
        entries.insert(
            scope.to_string(),
            SnapshotEntry {
                rev,
                data,
                timestamp: now,
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
        if self.options.ttl_secs.is_none() {
            return 0;
        }
        let mut guard = self.entries.write();
        let before = guard.len();
        guard.retain(|_, entry| !self.is_expired(entry));
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

/// Whether an entry received at `timestamp` is expired at `now` for a ttl of
/// `ttl_ms` milliseconds. The comparison is inclusive — expired once the
/// elapsed time *reaches* the ttl — so `ttl_ms == 0` is expired immediately
/// rather than for the rest of the current millisecond. Elapsed time is
/// clamped at zero, so a future timestamp (clock skew) never expires early.
fn is_expired_at(ttl_ms: u64, timestamp: i64, now: i64) -> bool {
    now.saturating_sub(timestamp).max(0) as u64 >= ttl_ms
}

/// Write/read handle of the snapshot caches, obtained via [`Engine::snapshot`](crate::Engine::snapshot).
///
/// Feed adapters (message channels) call [`upsert`](Self::upsert) /
/// [`remove`](Self::remove) when data arrives; the effect seam reads the
/// same store at each prepare. Sealing is per effect and never persisted —
/// see the [module docs](self). `upsert` auto-registers the target with
/// default options when it does not exist yet, so a feed never drops data on
/// an unregistered name — register explicitly first when a non-default
/// policy or scope keys are needed. `remove` acts on an existing target
/// only: a tombstone for a name the engine never registered is a wiring
/// error, not a silent no-op.
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

    /// Feed a new value for `name`/`scope`. Auto-registers the target with
    /// [`SnapshotOptions::default`] when missing, so the write only fails
    /// when those options are rejected (see [`SnapshotOptions::validate`]).
    /// Revisions are monotonic per scope: a value whose `rev` is not newer
    /// than the cached one is ignored (a stale revision cannot roll the
    /// scope back). An auto-registered target is always pure push — a
    /// fetch-capable variable can only come from a `pack:publish` carrying
    /// `options.var` metadata.
    pub fn upsert(&self, name: &str, scope: &str, rev: u64, data: Vars) -> crate::Result<()> {
        let store = match self.runtime.snapshot_store(name) {
            Some(store) => store,
            None => self
                .runtime
                .register_snap(name, SnapshotOptions::default(), None)?,
        };
        store.upsert(scope, rev, data);
        Ok(())
    }

    /// Remove the value of `name`/`scope` (tombstone). Fails when the target
    /// is not registered — there is nothing to tombstone, and a caller that
    /// believes it deleted a target should hear about the mismatch.
    pub fn remove(&self, name: &str, scope: &str) -> crate::Result<()> {
        let store = self
            .runtime
            .snapshot_store(name)
            .ok_or_else(|| ActError::Runtime(format!("snapshot '{name}' is not registered")))?;
        store.remove(scope);
        Ok(())
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

/// Single-flight coordinator of the fetch-on-miss pull path.
///
/// One prepare per `(target, scope)` becomes the **leader**: it emits the
/// request message (a `type: "req"` event to the matching channels) and waits
/// a bounded time; concurrent prepares of the same scope **join** the same
/// wait instead of calling out again. The reply itself lands through the
/// `snap:reply` action — the same rev guard as a push feed — and a woken
/// prepare simply re-reads the store, so a late or stale reply cannot poison
/// anything.
#[derive(Clone, Default)]
pub(crate) struct SnapFetchRegistry {
    pending: ShareLock<HashMap<(String, String), Arc<FetchWaiter>>>,
}

#[derive(Debug)]
struct FetchWaiter {
    req_id: String,
    tx: tokio::sync::watch::Sender<bool>,
    /// how many prepares still wait on this slot; the last one out removes it
    waiters: std::sync::atomic::AtomicUsize,
}

impl SnapFetchRegistry {
    pub(crate) fn new() -> Self {
        Self {
            pending: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// One bounded fetch. `true` = a reply landed: the caller re-reads the
    /// store. `emit` runs once, on the leader only, with the request id.
    pub(crate) async fn fetch(
        &self,
        target: &str,
        scope: &str,
        timeout: std::time::Duration,
        emit: impl FnOnce(&str),
    ) -> bool {
        let key = (target.to_string(), scope.to_string());
        let (waiter, rx, is_leader) = {
            let mut pending = self.pending.write();
            match pending.get(&key) {
                Some(waiter) => (waiter.clone(), waiter.tx.subscribe(), false),
                None => {
                    let waiter = Arc::new(FetchWaiter {
                        req_id: crate::utils::shortid(),
                        tx: tokio::sync::watch::channel(false).0,
                        waiters: std::sync::atomic::AtomicUsize::new(0),
                    });
                    let rx = waiter.tx.subscribe();
                    pending.insert(key.clone(), waiter.clone());
                    (waiter, rx, true)
                }
            }
        };
        waiter
            .waiters
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let wait = |mut rx: tokio::sync::watch::Receiver<bool>| async move {
            // a reply may have landed before this waiter subscribed: the
            // watch channel keeps the latest value, so read it first
            if *rx.borrow_and_update() {
                return true;
            }
            loop {
                if rx.changed().await.is_err() {
                    return false;
                }
                if *rx.borrow_and_update() {
                    return true;
                }
            }
        };
        let replied = if is_leader {
            emit(&waiter.req_id);
            let replied = tokio::time::timeout(timeout, wait(rx))
                .await
                .unwrap_or(false);
            tracing::debug!(target, scope, replied, "snapshot fetch settled");
            replied
        } else {
            let replied = tokio::time::timeout(timeout, wait(rx))
                .await
                .unwrap_or(false);
            tracing::debug!(target, scope, replied, "snapshot fetch joiner settled");
            replied
        };
        self.release(&key, &waiter);
        replied
    }

    /// The leaving prepare decrements the slot's waiter count; the last one
    /// out removes the slot — but only if it is still that exact waiter (a
    /// later leader may have replaced it after a timeout).
    fn release(&self, key: &(String, String), waiter: &Arc<FetchWaiter>) {
        if waiter
            .waiters
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst)
            == 1
        {
            let mut pending = self.pending.write();
            if pending
                .get(key)
                .map(|w| Arc::ptr_eq(w, waiter))
                .unwrap_or(false)
            {
                pending.remove(key);
            }
        }
    }

    /// A `snap:reply` landed for `req_id`: mark the waiter replied (the watch
    /// value persists, so a waiter that subscribes later still reads it) and
    /// wake whoever waits. When no fetch is counting on the slot (a request
    /// announced for a reply that landed before any prepare joined), the
    /// slot is released here — the value is in the store, and the next miss
    /// starts a fresh request. `false` — unknown request (a late reply after
    /// the leader gave up); the value is still cached, only nobody waits.
    pub(crate) fn resolve(&self, req_id: &str) -> bool {
        let waiter = self
            .pending
            .read()
            .iter()
            .find(|(_, waiter)| waiter.req_id == req_id)
            .map(|(key, waiter)| (key.clone(), waiter.clone()));
        match waiter {
            Some((key, waiter)) => {
                // the send succeeds only with a live receiver; the stored
                // value is what a later subscriber reads either way
                let _ = waiter.tx.send(true);
                if waiter.waiters.load(std::sync::atomic::Ordering::SeqCst) == 0 {
                    let mut pending = self.pending.write();
                    if pending
                        .get(&key)
                        .map(|w| Arc::ptr_eq(w, &waiter))
                        .unwrap_or(false)
                    {
                        pending.remove(&key);
                    }
                }
                true
            }
            None => false,
        }
    }

    #[cfg(test)]
    pub(crate) fn preannounce(&self, target: &str, scope: &str) -> String {
        let req_id = crate::utils::shortid();
        let waiter = Arc::new(FetchWaiter {
            req_id: req_id.clone(),
            tx: tokio::sync::watch::channel(false).0,
            waiters: std::sync::atomic::AtomicUsize::new(0),
        });
        self.pending
            .write()
            .insert((target.to_string(), scope.to_string()), waiter);
        req_id
    }

    #[cfg(test)]
    pub(crate) fn pending_scopes(&self) -> Vec<(String, String)> {
        self.pending.read().keys().cloned().collect()
    }
}

/// Emit one fetch request for `target`/`scope` and wait a bounded time for a
/// reply. `true` — a provider answered through `snap:reply`: re-read the
/// store. The request carries only names (`target`, `scope`, the run's pid
/// and the sealed owner context for provider self-filtering) — never sealed
/// data.
pub(crate) async fn snap_fetch(
    runtime: &Runtime,
    pid: &str,
    owner: &crate::acl::ScopePolicy,
    target: &str,
    scope: &str,
    timeout: std::time::Duration,
) -> bool {
    let registry = runtime.fetch_registry();
    let context: serde_json::Value = owner
        .context
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::from(v.as_str())))
        .collect::<serde_json::Map<String, serde_json::Value>>()
        .into();
    let emit = |req_id: &str| {
        let mut msg = Message {
            id: req_id.to_string(),
            r#type: "req".to_string(),
            name: "snap_fetch".to_string(),
            pid: pid.to_string(),
            uses: Some(target.to_string()),
            state: MessageState::Created,
            timestamp: time::timestamp(),
            ..Default::default()
        };
        msg.inputs.set("scope", scope);
        msg.inputs.set("req_id", req_id);
        msg.inputs.set("options", context.clone());
        runtime.emitter().emit_req(&msg);
        tracing::debug!(pid, target, scope, req_id, "snapshot fetch requested");
    };
    registry.fetch(target, scope, timeout, emit).await
}

/// The variable registration a `pack:publish` carries: the `options.var`
/// metadata turns the published package into the snapshot variable's
/// provider and registers the target under the given pull parameters.
/// `Ok(None)` — the package declares no variable (the common case: every
/// `acts.core.*` package and any package with arbitrary options).
pub(crate) fn variable_target(
    pack: &crate::data::Package,
) -> crate::Result<Option<(String, SnapshotOptions)>> {
    let Some(raw) = &pack.options else {
        return Ok(None);
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        // options are free-form; not a JSON object simply means "no variable"
        return Ok(None);
    };
    let Some(var) = value.get("var").and_then(|v| v.as_str()).map(str::trim) else {
        return Ok(None);
    };
    if var.is_empty() || var.len() > 64 || var.contains('/') {
        return Err(ActError::Config(format!(
            "package '{}' declares an invalid snapshot variable '{var}': \
             it must be 1..=64 bytes without a '/'",
            pack.id
        )));
    }
    if pack.run_as != ActRunAs::Irq {
        return Err(ActError::Config(format!(
            "package '{}' declares snapshot variable '{var}' but its run_as \
             cannot answer fetches; only 'irq' providers can",
            pack.id
        )));
    }
    let ttl_secs = value.get("ttl_secs").and_then(|v| v.as_u64());
    let on_missing = match value.get("on_missing").and_then(|v| v.as_str()) {
        None | Some("skip") => MissingParamAction::Skip,
        Some("error") => MissingParamAction::Error,
        Some(other) => {
            return Err(ActError::Config(format!(
                "package '{}' declares snapshot variable '{var}' with an \
                 invalid on_missing '{other}' (skip|error)",
                pack.id
            )));
        }
    };
    let scope = match value.get("scope") {
        None => Vec::new(),
        Some(v) => serde_json::from_value::<Vec<String>>(v.clone()).map_err(|err| {
            ActError::Config(format!(
                "package '{}' declares snapshot variable '{var}' with an \
                 invalid scope array: {err}",
                pack.id
            ))
        })?,
    };
    let options = SnapshotOptions {
        policy: SnapshotPolicy::default(),
        scope,
        on_missing,
        ttl_secs,
        fetch: Some(FetchOptions {
            timeout_ms: value
                .get("timeout_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or_else(|| FetchOptions::default().timeout_ms),
        }),
    };
    options.validate()?;
    Ok(Some((var.to_string(), options)))
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

/// Validate that every scope param of `options` resolves on the task chain,
/// returning the raw values in order, or the missing names.
pub(crate) fn resolve_scope_params(
    task: &crate::scheduler::ScopeTask,
    scope: &std::sync::Arc<crate::scheduler::Scope>,
    options: &SnapshotOptions,
) -> std::result::Result<Vec<serde_json::Value>, Vec<String>> {
    let mut values = Vec::with_capacity(options.scope.len());
    let mut missing = Vec::new();
    for p in &options.scope {
        match task.find::<serde_json::Value>(scope, p) {
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
    use crate::scheduler::shell::snapshot::SnapshotRegistry;
    use std::sync::Barrier;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

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
    fn store_upsert_ignores_stale_and_duplicate_rev() {
        let store = SnapshotStore::new(SnapshotOptions::default());
        store.upsert("s1", 2, Vars::new().with("a", 2));

        // a late delivery of an older revision must not roll the entry back
        store.upsert("s1", 1, Vars::new().with("a", 1));
        let entry = store.get("s1").unwrap();
        assert_eq!(entry.rev, 2);
        assert_eq!(entry.data.get::<i32>("a").unwrap(), 2);

        // an equal revision is an idempotent replay: the cached value stays
        store.upsert("s1", 2, Vars::new().with("a", 99));
        let entry = store.get("s1").unwrap();
        assert_eq!(entry.rev, 2);
        assert_eq!(entry.data.get::<i32>("a").unwrap(), 2);

        // a newer revision still wins
        store.upsert("s1", 3, Vars::new().with("a", 3));
        let entry = store.get("s1").unwrap();
        assert_eq!(entry.rev, 3);
        assert_eq!(entry.data.get::<i32>("a").unwrap(), 3);
    }

    #[test]
    fn store_upsert_replay_refreshes_ttl_basis() {
        let store = SnapshotStore::new(SnapshotOptions::default().with_ttl(1));
        store.upsert("s1", 1, Vars::new().with("a", 1));
        // age the entry, then replay the same revision
        {
            let mut entries = store.entries.write();
            entries.get_mut("s1").unwrap().timestamp = 1;
        }
        store.upsert("s1", 1, Vars::new().with("a", 2));

        let entry = store.get("s1").unwrap();
        assert_eq!(entry.rev, 1);
        assert_eq!(entry.data.get::<i32>("a").unwrap(), 1);
        assert!(entry.timestamp > 1, "replay must refresh the ttl basis");
    }

    #[test]
    fn store_upsert_concurrent_revs_keep_max() {
        let store = Arc::new(SnapshotStore::new(SnapshotOptions::default()));
        let barrier = Arc::new(Barrier::new(8));
        let mut handles = Vec::new();
        // every revision is attempted at the same instant: the write lock
        // decides the arrival order, the cache must still end at the max
        for rev in 1..=8u64 {
            let store = store.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                store.upsert("s1", rev, Vars::new().with("rev", rev as i64));
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }

        let entry = store.get("s1").unwrap();
        assert_eq!(entry.rev, 8);
        assert_eq!(entry.data.get::<i64>("rev").unwrap(), 8);
    }

    #[test]
    fn missing_err_format() {
        let err = missing_err("profile", &["unit".to_string(), "project".to_string()]);
        let text = err.to_string();
        assert!(text.contains("profile"), "got: {text}");
        assert!(text.contains("unit"), "got: {text}");
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

    /// Age an entry so the next read sees it as expired, without sleeping.
    fn age(store: &SnapshotStore, scope: &str) {
        store.entries.write().get_mut(scope).unwrap().timestamp = 1;
    }

    #[test]
    fn expired_read_does_not_drop_a_concurrent_refresh() {
        let store = SnapshotStore::new(SnapshotOptions::default().with_ttl(1));
        store.upsert("s1", 1, Vars::new().with("a", 1));
        age(&store, "s1");

        // the reader observes the expired entry...
        let observed = store.entries.read().get("s1").cloned().unwrap();
        // ...a feed refreshes the scope before the reader evicts...
        store.upsert("s1", 2, Vars::new().with("a", 2));
        // ...and the eviction must leave the refresh alone: the reader gets
        // the new value instead of the entry being dropped
        let got = store.evict_expired("s1", &observed).unwrap();
        assert_eq!(got.rev, 2);
        assert_eq!(got.data.get::<i32>("a").unwrap(), 2);
        assert_eq!(store.entries.read().get("s1").unwrap().rev, 2);

        // an unchanged entry is still evicted by the reader
        age(&store, "s1");
        let observed = store.entries.read().get("s1").cloned().unwrap();
        assert!(store.evict_expired("s1", &observed).is_none());
        assert!(store.entries.read().get("s1").is_none());
    }

    #[test]
    fn expired_read_and_refresh_race_keeps_the_refresh() {
        let store = Arc::new(SnapshotStore::new(SnapshotOptions::default().with_ttl(1)));
        for i in 0..64 {
            let scope = format!("s{i}");
            store.upsert(&scope, 1, Vars::new().with("a", 1));
            age(&store, &scope);

            let barrier = Arc::new(Barrier::new(2));
            let reader = {
                let store = store.clone();
                let scope = scope.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    store.get(&scope);
                })
            };
            let writer = {
                let store = store.clone();
                let scope = scope.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    store.upsert(&scope, 2, Vars::new().with("a", 2));
                })
            };
            reader.join().unwrap();
            writer.join().unwrap();

            // whichever order the two steps interleaved in, the refresh is
            // never lost to the expired read
            let entry = store.get(&scope).expect("refresh must survive the read");
            assert_eq!(entry.rev, 2);
            assert_eq!(entry.data.get::<i32>("a").unwrap(), 2);
        }
    }

    #[test]
    fn ttl_validation_bounds() {
        assert!(SnapshotOptions::default().with_ttl(0).validate().is_ok());
        assert!(
            SnapshotOptions::default()
                .with_ttl(MAX_TTL_SECS)
                .validate()
                .is_ok()
        );
        assert!(
            SnapshotOptions::default()
                .with_ttl(MAX_TTL_SECS + 1)
                .validate()
                .is_err()
        );
        assert!(
            SnapshotOptions::default()
                .with_ttl(u64::MAX)
                .validate()
                .is_err()
        );
    }

    #[test]
    fn ttl_expiry_boundary_is_inclusive() {
        // expired once the elapsed time reaches the ttl
        assert!(is_expired_at(1000, 1_000, 2_000));
        assert!(!is_expired_at(1000, 1_000, 1_999));
        // zero ttl: no valid window, expired at the instant of the read
        assert!(is_expired_at(0, 1_000, 1_000));
        // a future timestamp (clock skew) never expires early
        assert!(!is_expired_at(1000, 2_000, 1_000));
    }

    #[test]
    fn ttl_zero_expires_on_read_and_purge() {
        let store = SnapshotStore::new(SnapshotOptions::default().with_ttl(0));
        store.upsert("s1", 1, Vars::new().with("a", 1));
        assert!(store.get("s1").is_none(), "ttl 0 is never a valid window");
        assert_eq!(store.purge_expired(), 0, "the read already evicted it");

        store.upsert("s1", 2, Vars::new().with("a", 2));
        assert_eq!(store.purge_expired(), 1);
    }

    #[test]
    fn out_of_range_ttl_never_panics_or_expires() {
        // direct construction bypasses `validate`: the store must stay total
        // for any u64 and never invent a build-profile-dependent expiry
        for ttl in [u64::MAX, MAX_TTL_SECS + 1] {
            let store = SnapshotStore::new(SnapshotOptions::default().with_ttl(ttl));
            store.upsert("s1", 1, Vars::new().with("a", 1));
            age(&store, "s1");
            assert!(
                store.get("s1").is_some(),
                "ttl {ttl} must not expire a live entry"
            );
            assert_eq!(store.purge_expired(), 0, "ttl {ttl}");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn the_manager_registers_validates_and_services_scopes() {
        let runtime = Runtime::new(&crate::Config::default(), None).unwrap();
        let manager = SnapshotManager::new(&runtime);

        // A valid variable registration, then the full write/read/remove
        // surface.
        runtime
            .register_snap("profile", SnapshotOptions::default(), None)
            .unwrap();
        assert!(manager.read("profile", "u1").is_none());
        assert!(manager.list("profile").is_empty());
        manager
            .upsert("profile", "u1", 1, Vars::new().with("name", "ada"))
            .unwrap();
        assert_eq!(manager.list("profile").len(), 1);
        assert_eq!(
            manager
                .read("profile", "u1")
                .unwrap()
                .data
                .get::<String>("name")
                .unwrap(),
            "ada"
        );
        manager.remove("profile", "u1").unwrap();
        assert!(manager.read("profile", "u1").is_none());

        // `upsert` auto-registers an unknown target; `remove` refuses it.
        manager.upsert("auto", "s", 1, Vars::new()).unwrap();
        assert!(manager.list("auto").len() == 1);
        assert!(manager.remove("never-registered", "s").is_err());
        assert!(manager.read("never-registered", "s").is_none());
        assert!(manager.list("never-registered").is_empty());

        // Invalid options are refused at registration (both paths).
        let invalid = SnapshotOptions::default().with_ttl(MAX_TTL_SECS + 1);
        assert!(runtime.register_snap("bad", invalid, None).is_err());
        assert!(
            runtime
                .register_snap(
                    "bad",
                    SnapshotOptions::default().with_ttl(1),
                    Some("acts.test.bad")
                )
                .is_ok()
        );

        runtime.close().await;
    }

    /// A same-revision refresh replaces only the ttl basis: the eviction must
    /// recognise the entry as refreshed by its timestamp and keep it.
    #[test]
    fn an_expired_read_keeps_a_same_revision_refresh() {
        let store = SnapshotStore::new(SnapshotOptions::default().with_ttl(1));
        store.upsert("s1", 1, Vars::new().with("a", 1));
        let mut observed = store.entries.read().get("s1").cloned().unwrap();
        observed.timestamp = 1;
        // A replay of the same revision refreshes the ttl basis (and keeps
        // the cached value).
        store.upsert("s1", 1, Vars::new().with("a", 2));
        let got = store
            .evict_expired("s1", &observed)
            .expect("a refreshed entry survives the read");
        assert_eq!(got.rev, 1);
        assert_eq!(got.data.get::<i32>("a").unwrap(), 1);
        assert!(store.get("s1").is_some());
    }

    #[test]
    fn fetch_timeout_clamps_to_a_usable_window() {
        assert_eq!(FetchOptions::default().timeout().as_millis(), 3000);
        assert_eq!(
            FetchOptions { timeout_ms: 50 }.timeout().as_millis(),
            100,
            "below the floor no remote round trip is reliable"
        );
        assert_eq!(
            FetchOptions { timeout_ms: 60_000 }.timeout().as_millis(),
            30_000,
            "above the ceiling an outage would hold every cold act"
        );
        assert_eq!(
            FetchOptions { timeout_ms: 1234 }.timeout().as_millis(),
            1234
        );
    }

    #[tokio::test]
    async fn concurrent_fetches_share_one_request_and_one_reply() {
        let registry = SnapFetchRegistry::new();
        let emits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let leader_registry = registry.clone();
        let leader_emits = emits.clone();
        let leader = tokio::spawn(async move {
            leader_registry
                .fetch("profile", "u9", Duration::from_secs(5), |_| {
                    leader_emits.fetch_add(1, Ordering::SeqCst);
                })
                .await
        });
        // give the leader the slot, then join as the concurrent prepare
        tokio::time::sleep(Duration::from_millis(20)).await;
        let joiner = {
            let registry = registry.clone();
            tokio::spawn(async move {
                registry
                    .fetch("profile", "u9", Duration::from_secs(5), |_| {
                        panic!("a joiner must not emit its own request");
                    })
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;

        // exactly one request went out; its reply wakes both waiters
        assert_eq!(emits.load(Ordering::SeqCst), 1);
        let req_id = registry
            .pending
            .read()
            .values()
            .next()
            .unwrap()
            .req_id
            .clone();
        assert!(registry.resolve(&req_id));
        assert!(leader.await.unwrap());
        assert!(joiner.await.unwrap());
        assert!(registry.pending_scopes().is_empty());
    }

    #[tokio::test]
    async fn a_timed_out_fetch_frees_the_slot_and_answers_false() {
        let registry = SnapFetchRegistry::new();
        let replied = registry
            .fetch("profile", "u9", Duration::from_millis(80), |_| {})
            .await;
        assert!(!replied);
        assert!(
            registry.pending_scopes().is_empty(),
            "the timed-out leader must release the slot"
        );

        // a reply for the dead request resolves nothing
        assert!(!registry.resolve("no-such-req"));

        // the freed slot accepts a fresh fetch
        let replied = registry
            .fetch("profile", "u9", Duration::from_millis(80), |_| {})
            .await;
        assert!(!replied);
    }

    #[test]
    fn register_snap_conflicts_and_provider_duplicates_are_refused() {
        let registry = SnapshotRegistry::new();
        let base = SnapshotOptions {
            fetch: Some(FetchOptions { timeout_ms: 3000 }),
            ..SnapshotOptions::default()
        };
        registry
            .register_snap("profile", base.clone(), Some("acts.clientA.profile"))
            .unwrap();

        // the same name **and** provider twice is the one hard error: a
        // provider registers a variable once
        let Err(err) =
            registry.register_snap("profile", base.clone(), Some("acts.clientA.profile"))
        else {
            panic!("a duplicate provider registration must be refused");
        };
        assert!(err.contains("already registered by provider"), "{err}");
        assert_eq!(registry.providers("profile").len(), 1);

        // identical parameters from a **different** provider: the second
        // package joins the provider list — same-name variables are
        // distinguished by their providers
        registry
            .register_snap("profile", base.clone(), Some("acts.clientB.profile"))
            .unwrap();
        // the global registration (`None`) is yet another entry: the default
        // every run reads unless its context names a provider
        registry
            .register_snap("profile", base.clone(), None)
            .unwrap();
        // and the global duplicate is refused just the same
        let Err(err) = registry.register_snap("profile", base.clone(), None) else {
            panic!("a duplicate global registration must be refused");
        };
        assert!(err.contains("provider 'global'"), "{err}");
        assert_eq!(
            registry.providers("profile"),
            vec![
                "acts.clientA.profile".to_string(),
                "acts.clientB.profile".to_string(),
                String::new(),
            ]
        );
        // anything else: refused — the first publisher fixes the pull shape
        let Err(err) = registry.register_snap(
            "profile",
            SnapshotOptions {
                fetch: Some(FetchOptions { timeout_ms: 5000 }),
                ..SnapshotOptions::default()
            },
            Some("acts.clientC.profile"),
        ) else {
            panic!("conflicting parameters must be refused");
        };
        assert!(err.contains("different parameters"), "{err}");
        // a push target collides with a fetch target just the same
        let Err(err) = registry.register_snap(
            "profile",
            SnapshotOptions::default(),
            Some("acts.clientD.profile"),
        ) else {
            panic!("a push registration over a fetch variable must be refused");
        };
        assert!(err.contains("different parameters"), "{err}");
        assert_eq!(
            registry.providers("profile").len(),
            3,
            "a refused registration must not join the provider list"
        );
    }

    #[test]
    fn variable_target_parses_the_publish_metadata() {
        let pack = |options: Option<serde_json::Value>, run_as: ActRunAs| crate::data::Package {
            id: "acts.clientA.profile".into(),
            options: options.map(|v| v.to_string()),
            run_as,
            ..Default::default()
        };
        // no options, or options without `var`: not a variable package
        assert!(
            variable_target(&pack(None, ActRunAs::Irq))
                .unwrap()
                .is_none()
        );
        assert!(
            variable_target(&pack(Some(serde_json::json!({"other": 1})), ActRunAs::Irq))
                .unwrap()
                .is_none()
        );
        // non-JSON options are free-form package data, not an error
        let mut free_form = pack(Some(serde_json::json!([])), ActRunAs::Irq);
        free_form.options = Some("not json".to_string());
        assert!(variable_target(&free_form).unwrap().is_none());

        // full metadata
        let (var, options) = variable_target(&pack(
            Some(serde_json::json!({
                "var": "profile",
                "ttl_secs": 300,
                "timeout_ms": 1500,
                "on_missing": "error",
                "scope": ["tenant", "unit"]
            })),
            ActRunAs::Irq,
        ))
        .unwrap()
        .unwrap();
        assert_eq!(var, "profile");
        assert_eq!(options.ttl_secs, Some(300));
        assert_eq!(options.on_missing, MissingParamAction::Error);
        assert_eq!(
            options.scope,
            vec!["tenant".to_string(), "unit".to_string()]
        );
        assert_eq!(options.fetch, Some(FetchOptions { timeout_ms: 1500 }));

        // minimal metadata: the defaults
        let (var, options) = variable_target(&pack(
            Some(serde_json::json!({ "var": "security" })),
            ActRunAs::Irq,
        ))
        .unwrap()
        .unwrap();
        assert_eq!(var, "security");
        assert_eq!(
            options,
            SnapshotOptions {
                fetch: Some(FetchOptions::default()),
                ..SnapshotOptions::default()
            }
        );
        assert_eq!(
            options.fetch.as_ref().unwrap().timeout(),
            FetchOptions::default().timeout()
        );

        // refusals
        for (options, expected) in [
            (serde_json::json!({"var": ""}), "invalid snapshot variable"),
            (
                serde_json::json!({"var": "a/b"}),
                "invalid snapshot variable",
            ),
            (
                serde_json::json!({"var": "x", "on_missing": "boom"}),
                "invalid on_missing",
            ),
            (
                serde_json::json!({"var": "x", "scope": "tenant"}),
                "invalid scope array",
            ),
            (
                serde_json::json!({"var": "x", "ttl_secs": MAX_TTL_SECS + 1}),
                "exceeds the maximum",
            ),
        ] {
            let err = variable_target(&pack(Some(options), ActRunAs::Irq)).unwrap_err();
            assert!(err.to_string().contains(expected), "{err}");
        }
        // only an irq package can answer fetches
        for run_as in [ActRunAs::Func, ActRunAs::Msg] {
            let err = variable_target(&pack(Some(serde_json::json!({ "var": "profile" })), run_as))
                .unwrap_err();
            assert!(err.to_string().contains("run_as"), "{err}");
        }
        // a long name is refused, a 64-byte name passes
        let err = variable_target(&pack(
            Some(serde_json::json!({ "var": "x".repeat(65) })),
            ActRunAs::Irq,
        ))
        .unwrap_err();
        assert!(
            err.to_string().contains("invalid snapshot variable"),
            "{err}"
        );
        variable_target(&pack(
            Some(serde_json::json!({ "var": "x".repeat(64) })),
            ActRunAs::Irq,
        ))
        .unwrap()
        .unwrap();
    }
}
