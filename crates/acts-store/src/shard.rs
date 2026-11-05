//! A consistent-hash sharded `KvStore`.
//!
//! One physical backend is still the unit of durability and atomicity; this
//! wrapper spreads the engine's *write* load over several of them. The unit of
//! placement is the process id: every key of one process — its aggregate
//! family, derived indexes, task/vars/message/delivery rows and the index rows
//! that point at them — hashes to the same physical shard, so a process's
//! guarded batch stays a single-shard transaction. Keys that belong to no
//! process (models, packages, ACL rows, the database lease) hash through the
//! same ring under a fixed global token and are not split.
//!
//! # Ring
//!
//! [`DEFAULT_VIRTUAL_NODES`] (1024) virtual-node slots are laid around the
//! XXH3-64 ring. Each slot is won by the active shard with the highest
//! deterministic score for that slot, so a shard owns about 1024 / N slots.
//! A pid's hash chooses the first slot at or after it; a node addition or
//! removal therefore changes only the slots the shard wins or loses (about
//! 1/(N+1) of the key space), not every pid. Slot positions and scores are
//! derived from the shard's configured id (not its position), so reordering
//! the shard list does not move data.
//!
//! # Ordered queries
//!
//! [`KvStore::scan_prefix`] fans out to every shard (active and draining),
//! then merges the per-shard ordered streams by key. The merge is the global
//! key order — the same order a single backend would return — and duplicate
//! copies left by a half-finished migration are resolved in favor of the
//! ring's current owner. A reverse scan sorts ascending, deduplicates, then
//! reverses, so `is_rev` is also a global order rather than a per-shard one.
//! `read_view` delegates to one shard when the whole family maps to it (the
//! normal engine case, and therefore still one native snapshot); a view that
//! spans shards samples until the merged answer is stable.
//!
//! # Node add/remove
//!
//! [`ShardedStore::rebalance`] copies every key to its current owner, then
//! deletes stale copies, in that order. A shard being removed is configured
//! with [`Shard::draining`]: it stays readable (and is included in scans and
//! point-read fallback) but receives no new writes until rebalance has moved
//! its keys. Once its copy count is zero it can be omitted from the next
//! configuration. Reads fall back to the other shards on a miss while a
//! migration is in flight (`ShardConfig::search_on_miss`); run rebalance before
//! accepting writes against the new membership, because a guarded write reads
//! its guard on the new owner and cannot see a not-yet-moved row.
//!
//! # Cross-shard batches
//!
//! A batch whose keys resolve to different shards cannot be atomic: the
//! physical databases have no shared transaction. [`KvStore::batch`] refuses
//! it with an error instead of applying half of it. The engine's write paths
//! are per process and therefore single-shard; the one exception is the
//! database lease, which is a global row and is checked as a guard on every
//! fenced write. `FencedStore` over a multi-shard `ShardedStore` is therefore
//! refused by `acts-server` at startup: use `lease = false` with one server
//! per shard set, or coordinate writers outside the engine.
//!
//! # Backpressure
//!
//! Each shard has a [`WriteGate`] admitting `ShardConfig::max_in_flight_writes`
//! concurrent writes. While a shard's gate is full, `KvStore::saturated`
//! returns true; the engine's store-writer combines that with its own backlog
//! latch so producers stop admitting new work before a second unbounded queue
//! forms.

use crate::pressure::WriteGate;
use acts::{
    ActError, KvStore, Result, STABLE_READ_ATTEMPTS, ScanOperation, ScanOptions, StorageUsage,
    StoreBatchOp, StoreGuard, StoreView,
};
use futures::future::join_all;
use serde_json::Value as JsonValue;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};
use xxhash_rust::xxh3::xxh3_64;

/// Virtual-node slots on the consistent-hash ring. The slots are shared by
/// every active shard, so a membership change only reassigns the slots around
/// the changed node.
pub const DEFAULT_VIRTUAL_NODES: usize = 1024;

/// Concurrent writes one physical shard admits by default.
pub const DEFAULT_SHARD_WRITES: usize = 16;

/// The token non-process keys hash under. It is not a valid process id (the
/// engine refuses a pid containing `-`), so a process can never collide with
/// the global token by spelling.
const GLOBAL_TOKEN: &str = "\u{0}acts-global";

/// The collections whose rows belong to one process. `models`, `packages`,
/// `events`, the ACL collections and the engine's `kernel/...` families are
/// handled separately.
const PROCESS_COLLECTIONS: [&str; 5] = ["procs", "tasks", "vars", "messages", "deliveries"];

const AGGREGATE_PREFIX: &str = "kernel/aggregates/";
const INDEX_PREFIXES: [&str; 3] = [
    "kernel/indexes/outbox/",
    "kernel/indexes/recovery/",
    "kernel/indexes/ready/",
];
const RECOVERY_ENTRY_PREFIX: &str = "kernel/indexes/recovery-entries/";

/// Whether a shard receives writes (`Active`) or is only kept for reading
/// while [`ShardedStore::rebalance`] drains it (`Draining`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShardRole {
    Active,
    Draining,
}

/// One physical database in a [`ShardedStore`].
///
/// `id` is the stable identity the ring hashes; use the backend's path or
/// connection URL so adding or removing a shard does not move an unrelated
/// shard's slots. Two shards with the same id are refused.
pub struct Shard {
    id: String,
    store: Arc<dyn KvStore>,
    role: ShardRole,
}

impl Shard {
    /// An active shard.
    pub fn new(id: impl Into<String>, store: Arc<dyn KvStore>) -> Self {
        Self {
            id: id.into(),
            store,
            role: ShardRole::Active,
        }
    }

    /// A shard kept only as a migration source. It is readable and included in
    /// scans, but new writes route to active owners.
    pub fn draining(id: impl Into<String>, store: Arc<dyn KvStore>) -> Self {
        Self {
            id: id.into(),
            store,
            role: ShardRole::Draining,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn store(&self) -> &Arc<dyn KvStore> {
        &self.store
    }

    pub fn role(&self) -> ShardRole {
        self.role
    }
}

/// Tuning for [`ShardedStore`].
#[derive(Clone, Debug)]
pub struct ShardConfig {
    /// Concurrent writes admitted per physical shard.
    pub max_in_flight_writes: usize,
    /// Look for a key on the other shards when its routed shard does not have
    /// it. On by default so reads survive a membership change before
    /// [`ShardedStore::rebalance`] has run; after a migration this can be
    /// turned off to avoid a fan-out for genuinely absent keys.
    pub search_on_miss: bool,
}

impl Default for ShardConfig {
    fn default() -> Self {
        Self {
            max_in_flight_writes: DEFAULT_SHARD_WRITES,
            search_on_miss: true,
        }
    }
}

/// What [`ShardedStore::rebalance`] moved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RebalanceReport {
    /// Distinct keys observed on the physical shards.
    pub scanned: usize,
    /// Keys copied to their current owner (the owner had no copy).
    pub copied: usize,
    /// Stale copies deleted from non-owner shards.
    pub removed: usize,
    /// Keys whose copies disagreed with each other. The owner's copy wins; a
    /// non-owner duplicate is counted once per key, not per copy.
    pub conflicts: usize,
}

/// The ring: sorted `(token, active shard index)` slots.
#[derive(Debug)]
struct HashRing {
    tokens: Vec<(u64, usize)>,
}

impl HashRing {
    fn new(nodes: &[(usize, String)]) -> Self {
        let mut tokens = Vec::with_capacity(DEFAULT_VIRTUAL_NODES.max(nodes.len()));
        let slots = DEFAULT_VIRTUAL_NODES.max(nodes.len()).max(1);
        for slot in 0..slots {
            // The slot's position is independent of the shard list. Its owner
            // is the shard with the highest deterministic score for this slot,
            // so adding or removing a shard changes only the slots that shard
            // wins or loses — every other slot's token and owner stay put.
            let token = xxh3_64(format!("acts-slot\u{0}{slot}").as_bytes());
            let (node, _) = nodes
                .iter()
                .max_by_key(|(_, id)| xxh3_64(format!("{id}\u{0}{slot}").as_bytes()))
                .expect("a ring needs at least one active shard");
            tokens.push((token, *node));
        }
        tokens.sort_unstable_by_key(|(token, _)| *token);
        tokens.dedup_by_key(|(token, _)| *token);
        Self { tokens }
    }

    fn route(&self, token: &str) -> usize {
        let hash = xxh3_64(token.as_bytes());
        let index = self
            .tokens
            .partition_point(|(candidate, _)| *candidate < hash);
        let slot = if index == self.tokens.len() { 0 } else { index };
        self.tokens[slot].1
    }
}

/// Where one key belongs.
#[derive(Clone, Debug, PartialEq, Eq)]
enum KeyRoute {
    /// The key carries its process id directly (aggregate/index keys and the
    /// process collections' `pid` index and `procs` rows).
    Pid(String),
    /// A document row (or a non-`pid` index row) whose process id is learned
    /// from the stored row or from a sibling index row in the same batch.
    Data { collection: String, id: String },
    /// No process owner (models, packages, events, ACL rows, the lease).
    Global,
}

/// The three segments of a process collection key:
/// `{collection}-{field}-{encoded_value}-{id}`. `Data` is the
/// `{collection}-id-{id}` form.
enum ProcessKey<'a> {
    Data {
        collection: &'a str,
        id: &'a str,
    },
    Index {
        collection: &'a str,
        field: &'a str,
        value: &'a str,
        id: &'a str,
    },
}

/// Inverse of the collection layer's index-value encoding: alphanumerics pass
/// through, every other character is `=XX` (2 hex digits for byte-sized code
/// points, 6 for larger). A pid's `pid` index value is decoded so its shard is
/// the same as the hex-encoded aggregate key and the raw data-row id.
fn decode_key_value(encoded: &str) -> String {
    let mut output = String::with_capacity(encoded.len());
    let mut rest = encoded;
    while let Some(index) = rest.find('=') {
        output.push_str(&rest[..index]);
        rest = &rest[index + 1..];
        // Six digits encode a code point above 0xFF; their first two hex
        // digits are `00` for every BMP character. Pids are well inside that
        // range, so two digits is the common branch.
        let digits = if rest.len() >= 6 && rest.starts_with("00") {
            6
        } else if rest.len() >= 2 {
            2
        } else {
            0
        };
        if digits == 0 {
            output.push('=');
            continue;
        }
        let Some(code) = u32::from_str_radix(&rest[..digits], 16).ok() else {
            output.push('=');
            continue;
        };
        let Some(ch) = char::from_u32(code) else {
            output.push('=');
            continue;
        };
        output.push(ch);
        rest = &rest[digits..];
    }
    output.push_str(rest);
    output
}

fn process_key(key: &str) -> Option<ProcessKey<'_>> {
    let (collection, rest) = key.split_once('-')?;
    if !PROCESS_COLLECTIONS.contains(&collection) {
        return None;
    }
    if let Some(id) = rest.strip_prefix("id-") {
        return Some(ProcessKey::Data { collection, id });
    }
    // Values are encoded without `-`, so the first separator ends the field
    // and the second ends the value; the id (which MAY contain `-`) is the
    // whole remainder.
    let (field, rest) = rest.split_once('-')?;
    let (value, id) = rest.split_once('-')?;
    Some(ProcessKey::Index {
        collection,
        field,
        value,
        id,
    })
}

fn classify_key(key: &str) -> KeyRoute {
    if let Some(pid) = encoded_pid(key) {
        return KeyRoute::Pid(pid);
    }
    match process_key(key) {
        Some(ProcessKey::Data {
            collection: "procs",
            id,
        }) => KeyRoute::Pid(id.to_string()),
        Some(ProcessKey::Data { collection, id }) => KeyRoute::Data {
            collection: collection.to_string(),
            id: id.to_string(),
        },
        Some(ProcessKey::Index {
            field: "pid",
            value,
            ..
        }) => KeyRoute::Pid(decode_key_value(value)),
        Some(ProcessKey::Index {
            collection: "procs",
            id,
            ..
        }) => KeyRoute::Pid(id.to_string()),
        Some(ProcessKey::Index { collection, id, .. }) => KeyRoute::Data {
            collection: collection.to_string(),
            id: id.to_string(),
        },
        None => KeyRoute::Global,
    }
}

/// The document `(collection, id)` a key belongs to, if it is a process
/// collection key. Used to associate a data row with a sibling `pid` index row
/// in the same batch.
fn document_of_key(key: &str) -> Option<(String, String)> {
    match process_key(key)? {
        ProcessKey::Data { collection, id } | ProcessKey::Index { collection, id, .. } => {
            Some((collection.to_string(), id.to_string()))
        }
    }
}

fn direct_pid_of_key(key: &str) -> Option<String> {
    match classify_key(key) {
        KeyRoute::Pid(pid) => Some(pid),
        _ => None,
    }
}

/// Parse the hex-encoded process id after an aggregate/index prefix. The id is
/// whole bytes (`even` hex digits), so the parse stops at the first non-hex
/// character: `/` after a head id, `.` after a recovery-entry id, or the end.
fn encoded_pid(key: &str) -> Option<String> {
    let prefix = if key.starts_with(AGGREGATE_PREFIX) {
        AGGREGATE_PREFIX
    } else if key.starts_with(RECOVERY_ENTRY_PREFIX) {
        RECOVERY_ENTRY_PREFIX
    } else {
        INDEX_PREFIXES
            .iter()
            .find(|prefix| key.starts_with(**prefix))?
    };
    let rest = &key[prefix.len()..];
    let end = rest
        .find(|ch: char| !ch.is_ascii_hexdigit())
        .unwrap_or(rest.len());
    if end == 0 || end % 2 != 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(end / 2);
    let digits = &rest.as_bytes()[..end];
    for pair in digits.as_chunks::<2>().0 {
        let high = (pair[0] as char).to_digit(16)?;
        let low = (pair[1] as char).to_digit(16)?;
        bytes.push((high << 4 | low) as u8);
    }
    String::from_utf8(bytes).ok()
}

fn cache_key(collection: &str, id: &str) -> String {
    format!("{collection}\u{0}{id}")
}

fn pid_from_row(value: &[u8]) -> Option<String> {
    let value: JsonValue = serde_json::from_slice(value).ok()?;
    value
        .get("pid")
        .and_then(JsonValue::as_str)
        .filter(|pid| !pid.is_empty())
        .map(str::to_string)
}

/// One scan operation, cloned for each shard.
#[derive(Clone, Debug)]
enum ScanSpec {
    Eq(String),
    Ne(String),
    In(Vec<String>),
    Range(Option<String>, Option<String>),
}

impl ScanSpec {
    fn of(op: &ScanOperation, key: &str) -> Self {
        match op {
            ScanOperation::Eq => ScanSpec::Eq(key.to_string()),
            ScanOperation::Ne => ScanSpec::Ne(key.to_string()),
            ScanOperation::In { values } => ScanSpec::In(values.clone()),
            ScanOperation::Range { lower, upper } => ScanSpec::Range(lower.clone(), upper.clone()),
        }
    }

    /// The call arguments one shard receives: `key` is the API's separate
    /// value-prefix argument, `options` carries the operation and the
    /// field-level prefix.
    fn call(&self, prefix: &str, is_rev: bool) -> (String, ScanOptions) {
        let (key, op) = match self {
            ScanSpec::Eq(key) => (key.clone(), ScanOperation::Eq),
            ScanSpec::Ne(key) => (key.clone(), ScanOperation::Ne),
            ScanSpec::In(values) => (
                String::new(),
                ScanOperation::In {
                    values: values.clone(),
                },
            ),
            ScanSpec::Range(lower, upper) => (
                String::new(),
                ScanOperation::Range {
                    lower: lower.clone(),
                    upper: upper.clone(),
                },
            ),
        };
        let options = ScanOptions::new(op, prefix.to_string(), is_rev);
        (key, options)
    }
}

struct Inner {
    shards: Vec<Shard>,
    gates: Vec<Arc<WriteGate>>,
    active: Vec<usize>,
    ring: HashRing,
    config: ShardConfig,
    /// `(collection, id)` → process id, learned from stored rows and sibling
    /// index keys. A cache miss is handled by a shard search, never by
    /// guessing.
    ids: RwLock<HashMap<String, String>>,
}

impl Inner {
    fn learn_pid(&self, key: &str, value: &[u8]) {
        let KeyRoute::Data { collection, id } = classify_key(key) else {
            return;
        };
        if let Some(pid) = pid_from_row(value) {
            self.ids
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(cache_key(&collection, &id), pid);
        }
    }

    fn cached_pid(&self, collection: &str, id: &str) -> Option<String> {
        self.ids
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&cache_key(collection, id))
            .cloned()
    }

    fn route_key(&self, key: &str) -> Option<usize> {
        match classify_key(key) {
            KeyRoute::Pid(pid) => Some(self.ring.route(&pid)),
            KeyRoute::Data { collection, id } => self
                .cached_pid(&collection, &id)
                .map(|pid| self.ring.route(&pid)),
            KeyRoute::Global => Some(self.ring.route(GLOBAL_TOKEN)),
        }
    }

    /// The shard a prefix is wholly contained in, when the prefix itself names
    /// one process. A global prefix (`kernel/aggregates/`, `models-`, …) is
    /// `None` and must be scanned on every shard.
    fn prefix_shard(&self, prefix: &str) -> Option<usize> {
        match classify_key(prefix) {
            KeyRoute::Pid(pid) if !pid.is_empty() => Some(self.ring.route(&pid)),
            KeyRoute::Data { collection, id } if !id.is_empty() => self
                .cached_pid(&collection, &id)
                .map(|pid| self.ring.route(&pid)),
            _ => None,
        }
    }

    /// The single owning shard of one atomic batch, or an error when its keys
    /// span shards. Learning from the batch's own bytes (a data row's `pid`
    /// field, a sibling `pid` index row, a guard's expected row) is what lets a
    /// create/delete work before the id cache has ever seen the row.
    fn batch_shard(&self, ops: &[StoreBatchOp], guards: &[StoreGuard]) -> Result<usize> {
        for op in ops {
            if let StoreBatchOp::Put { key, value } = op {
                self.learn_pid(key, value);
            }
        }
        for guard in guards {
            if let Some(expected) = &guard.expected {
                self.learn_pid(&guard.key, expected);
            }
        }

        let mut sibling_pid: HashMap<String, String> = HashMap::new();
        for key in ops
            .iter()
            .map(|op| match op {
                StoreBatchOp::Put { key, .. } | StoreBatchOp::Delete { key } => key.as_str(),
            })
            .chain(guards.iter().map(|guard| guard.key.as_str()))
        {
            if let (Some(pid), Some((collection, id))) =
                (direct_pid_of_key(key), document_of_key(key))
            {
                sibling_pid.insert(cache_key(&collection, &id), pid);
            }
        }

        let mut target: Option<usize> = None;
        for key in ops
            .iter()
            .map(|op| match op {
                StoreBatchOp::Put { key, .. } | StoreBatchOp::Delete { key } => key.as_str(),
            })
            .chain(guards.iter().map(|guard| guard.key.as_str()))
        {
            let shard = match classify_key(key) {
                KeyRoute::Pid(pid) => self.ring.route(&pid),
                KeyRoute::Data { collection, id } => {
                    let cache = cache_key(&collection, &id);
                    let pid = self
                        .cached_pid(&collection, &id)
                        .or_else(|| sibling_pid.get(&cache).cloned())
                        .ok_or_else(|| {
                            ActError::Store(format!(
                                "sharded batch cannot route {key:?}: the row is absent from the \
                                 id cache and no sibling pid index or pid value names its process"
                            ))
                        })?;
                    self.ring.route(&pid)
                }
                KeyRoute::Global => self.ring.route(GLOBAL_TOKEN),
            };
            match target {
                Some(existing) if existing != shard => {
                    return Err(ActError::Store(format!(
                        "sharded store cannot commit one batch across physical shards \
                         (shard {existing} and shard {shard}): the batch's keys must share one \
                         process or all be non-process keys. Run ShardedStore::rebalance after a \
                         membership change before serving writes."
                    )));
                }
                Some(_) => {}
                None => target = Some(shard),
            }
        }
        target
            .ok_or_else(|| ActError::Store("sharded store cannot route an empty batch".to_string()))
    }

    async fn read_one(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let route = self.route_key(key);
        if let Some(index) = route {
            if let Some(value) = self.shards[index].store.one(key).await? {
                self.learn_pid(key, &value);
                return Ok(Some(value));
            }
            if !self.config.search_on_miss {
                return Ok(None);
            }
        }
        for (index, shard) in self.shards.iter().enumerate() {
            if route == Some(index) {
                continue;
            }
            if let Some(value) = shard.store.one(key).await? {
                self.learn_pid(key, &value);
                return Ok(Some(value));
            }
        }
        Ok(None)
    }

    async fn read_many(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>> {
        let mut values = vec![None; keys.len()];
        let mut routed: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        let mut unresolved = Vec::new();
        for (position, key) in keys.iter().enumerate() {
            match self.route_key(key) {
                Some(shard) => routed.entry(shard).or_default().push(position),
                None => unresolved.push(position),
            }
        }

        for (shard, positions) in &routed {
            let subset: Vec<String> = positions.iter().map(|&i| keys[i].clone()).collect();
            let found = self.shards[*shard].store.many(&subset).await?;
            for (&position, value) in positions.iter().zip(found) {
                if let Some(bytes) = &value {
                    self.learn_pid(&keys[position], bytes);
                }
                values[position] = value;
            }
        }

        if !unresolved.is_empty() {
            let subset: Vec<String> = unresolved.iter().map(|&i| keys[i].clone()).collect();
            for shard in &self.shards {
                let found = shard.store.many(&subset).await?;
                for (&position, value) in unresolved.iter().zip(found) {
                    if values[position].is_none()
                        && let Some(bytes) = &value
                    {
                        self.learn_pid(&keys[position], bytes);
                        values[position] = value;
                    }
                }
            }
        }

        if self.config.search_on_miss {
            for (position, value) in values.iter_mut().enumerate() {
                if value.is_none() {
                    *value = self.read_one(&keys[position]).await?;
                }
            }
        }
        Ok(values)
    }
}

/// A `KvStore` that shards process-owned keys over several physical backends.
pub struct ShardedStore {
    inner: Arc<Inner>,
}

impl ShardedStore {
    /// A store over `shards` with the default [`ShardConfig`].
    pub fn new(shards: Vec<Shard>) -> Result<Self> {
        Self::with_config(shards, ShardConfig::default())
    }

    /// A store over `shards` with explicit tuning.
    ///
    /// Fails when there is no active shard, when an id is empty, or when two
    /// shards share an id — an id collision would make two physical databases
    /// own the same ring slots.
    pub fn with_config(shards: Vec<Shard>, config: ShardConfig) -> Result<Self> {
        let mut ids = HashMap::new();
        for shard in &shards {
            if shard.id.is_empty() {
                return Err(ActError::Config(
                    "a sharded store needs a non-empty shard id".to_string(),
                ));
            }
            if ids.insert(shard.id.clone(), ()).is_some() {
                return Err(ActError::Config(format!(
                    "duplicate shard id {:?}: ids are the ring identity and must be unique",
                    shard.id
                )));
            }
        }
        let active: Vec<usize> = shards
            .iter()
            .enumerate()
            .filter_map(|(index, shard)| (shard.role == ShardRole::Active).then_some(index))
            .collect();
        if active.is_empty() {
            return Err(ActError::Config(
                "a sharded store needs at least one active shard".to_string(),
            ));
        }
        let nodes: Vec<(usize, String)> = active
            .iter()
            .map(|&index| (index, shards[index].id.clone()))
            .collect();
        let ring = HashRing::new(&nodes);
        let gates = shards
            .iter()
            .map(|_| WriteGate::new(config.max_in_flight_writes))
            .collect();
        Ok(Self {
            inner: Arc::new(Inner {
                shards,
                gates,
                active,
                ring,
                config,
                ids: RwLock::new(HashMap::new()),
            }),
        })
    }

    /// Number of physical shards, active and draining.
    pub fn shard_count(&self) -> usize {
        self.inner.shards.len()
    }

    /// Number of shards that receive writes.
    pub fn active_shard_count(&self) -> usize {
        self.inner.active.len()
    }

    /// Virtual-node slots on the ring.
    pub fn virtual_nodes(&self) -> usize {
        self.inner.ring.tokens.len()
    }

    /// The active shard `pid` hashes to.
    pub fn shard_for_pid(&self, pid: &str) -> usize {
        self.inner.ring.route(pid)
    }

    /// The shard a key currently routes to, or `None` when a document id has
    /// not been learned yet.
    pub fn shard_for_key(&self, key: &str) -> Option<usize> {
        self.inner.route_key(key)
    }

    /// Move every key to its current owner, then delete stale copies. Safe to
    /// run while the engine is stopped; reads may run meanwhile (they include
    /// every shard and prefer the owner's copy), but writes against a changed
    /// membership must wait until this returns.
    pub async fn rebalance(&self) -> Result<RebalanceReport> {
        let inner = &*self.inner;
        let mut found: BTreeMap<String, Vec<(usize, Vec<u8>)>> = BTreeMap::new();
        for (index, shard) in inner.shards.iter().enumerate() {
            let rows = shard
                .store
                .scan_prefix(
                    "",
                    ScanOptions::new(ScanOperation::Eq, String::new(), false),
                )
                .await?;
            for (key, value) in rows {
                found.entry(key).or_default().push((index, value));
            }
        }

        let mut copies: BTreeMap<usize, Vec<StoreBatchOp>> = BTreeMap::new();
        let mut deletes: BTreeMap<usize, Vec<StoreBatchOp>> = BTreeMap::new();
        let mut report = RebalanceReport {
            scanned: found.len(),
            ..RebalanceReport::default()
        };

        for (key, entries) in &found {
            for (_, value) in entries {
                inner.learn_pid(key, value);
            }
            let Some(owner) = inner.route_key(key) else {
                // An orphan row with no recognisable process and no data row
                // to learn from is left where it is rather than moved by a
                // guess.
                continue;
            };
            let owner_value = entries
                .iter()
                .find(|(index, _)| *index == owner)
                .map(|(_, value)| value);
            let first = &entries[0].1;
            let all_equal = entries.iter().all(|(_, value)| value == first);
            if !all_equal {
                report.conflicts += 1;
            }
            if owner_value.is_none() {
                copies.entry(owner).or_default().push(StoreBatchOp::Put {
                    key: key.clone(),
                    value: first.clone(),
                });
                report.copied += 1;
            }
            for (index, _) in entries {
                if *index != owner {
                    deletes
                        .entry(*index)
                        .or_default()
                        .push(StoreBatchOp::Delete { key: key.clone() });
                    report.removed += 1;
                }
            }
        }

        // Copies first, deletes second: a crash in the middle leaves duplicate
        // rows (the next pass removes them), never a missing row.
        for (index, ops) in &copies {
            apply_chunks(&inner.shards[*index].store, ops).await?;
        }
        for (index, ops) in &deletes {
            apply_chunks(&inner.shards[*index].store, ops).await?;
        }
        Ok(report)
    }
}

/// Apply one shard's rebalance ops in bounded batches. The shard is written
/// through its raw `KvStore`, not through [`ShardedStore`], so a copy is not
/// routed a second time.
async fn apply_chunks(store: &Arc<dyn KvStore>, ops: &[StoreBatchOp]) -> Result<()> {
    const CHUNK: usize = 256;
    for chunk in ops.chunks(CHUNK) {
        store.batch(chunk, &[]).await?;
    }
    Ok(())
}

#[async_trait::async_trait]
impl KvStore for ShardedStore {
    async fn one(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.inner.read_one(key).await
    }

    /// The sum of the physical shards' footprints. A shard that cannot size
    /// itself makes the combined answer unknown rather than understated.
    async fn usage(&self) -> Result<Option<StorageUsage>> {
        let mut used = 0u64;
        let mut measured = false;
        for shard in &self.inner.shards {
            if let Some(usage) = shard.store.usage().await? {
                match usage.used_bytes {
                    Some(bytes) => {
                        measured = true;
                        used = used.saturating_add(bytes);
                    }
                    None => {
                        return Ok(Some(StorageUsage {
                            kind: "sharded".to_string(),
                            used_bytes: None,
                        }));
                    }
                }
            }
        }
        Ok(Some(StorageUsage {
            kind: "sharded".to_string(),
            used_bytes: measured.then_some(used),
        }))
    }

    async fn flush(&self) -> Result<()> {
        let mut calls = Vec::with_capacity(self.inner.shards.len());
        for shard in &self.inner.shards {
            calls.push(shard.store.flush());
        }
        for result in join_all(calls).await {
            result?;
        }
        Ok(())
    }

    fn saturated(&self) -> bool {
        self.inner.gates.iter().any(|gate| gate.saturated())
            || self
                .inner
                .shards
                .iter()
                .any(|shard| shard.store.saturated())
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        self.inner.learn_pid(key, &value);
        let index = match classify_key(key) {
            KeyRoute::Data { .. } => self.inner.route_key(key).ok_or_else(|| {
                ActError::Store(format!(
                    "sharded put cannot route {key:?}: the row carries no pid and no sibling \
                     index row has taught the id cache its process"
                ))
            })?,
            _ => self
                .inner
                .route_key(key)
                .expect("pid and global keys always route"),
        };
        let _permit = self.inner.gates[index].acquire().await?;
        self.inner.shards[index].store.put(key, value).await
    }

    async fn delete(&self, key: &str) -> Result<()> {
        if let KeyRoute::Data { .. } = classify_key(key) {
            if let Some(index) = self.inner.route_key(key) {
                let _permit = self.inner.gates[index].acquire().await?;
                return self.inner.shards[index].store.delete(key).await;
            }
            // The id cache has not seen this row: delete every copy, since the
            // caller cannot name a shard and a stale copy would resurrect the
            // document on the next read.
            for (index, shard) in self.inner.shards.iter().enumerate() {
                if shard.store.one(key).await?.is_some() {
                    let _permit = self.inner.gates[index].acquire().await?;
                    shard.store.delete(key).await?;
                }
            }
            return Ok(());
        }
        let index = self
            .inner
            .route_key(key)
            .expect("pid and global keys always route");
        let _permit = self.inner.gates[index].acquire().await?;
        self.inner.shards[index].store.delete(key).await
    }

    async fn many(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        self.inner.read_many(keys).await
    }

    async fn read_stable(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let routes: Vec<Option<usize>> = keys.iter().map(|key| self.inner.route_key(key)).collect();
        if let Some(first) = routes[0]
            && routes.iter().all(|route| *route == Some(first))
        {
            return self.inner.shards[first].store.read_stable(keys).await;
        }
        for _ in 0..STABLE_READ_ATTEMPTS {
            let before = self.inner.read_many(keys).await?;
            let after = self.inner.read_many(keys).await?;
            if before == after {
                return Ok(before);
            }
            tokio::task::yield_now().await;
        }
        Err(ActError::Store(format!(
            "sharded store keys did not stay stable across {STABLE_READ_ATTEMPTS} samples"
        )))
    }

    /// One shard's native snapshot when the whole view maps to it (the normal
    /// engine case: one process's aggregate family). A view spanning shards
    /// samples the merged answer until it is stable, like the trait default.
    async fn read_view(&self, keys: &[String], prefix: &str) -> Result<StoreView> {
        let prefix_shard = self.inner.prefix_shard(prefix);
        let key_shard = if keys.is_empty() {
            None
        } else {
            let routes: Vec<Option<usize>> =
                keys.iter().map(|key| self.inner.route_key(key)).collect();
            routes[0].filter(|first| routes.iter().all(|route| *route == Some(*first)))
        };
        let target = if keys.is_empty() {
            prefix_shard
        } else {
            match (key_shard, prefix_shard) {
                // The keys and the prefix walk must both be on this shard: a
                // global prefix delegated to one shard would return only that
                // shard's part of the family.
                (Some(key), Some(prefix)) if key == prefix => Some(key),
                _ => None,
            }
        };
        if let Some(index) = target {
            return self.inner.shards[index].store.read_view(keys, prefix).await;
        }

        for _ in 0..STABLE_READ_ATTEMPTS {
            let before = self.read_stable(keys).await?;
            let entries = self
                .scan_prefix(
                    prefix,
                    ScanOptions::new(ScanOperation::Eq, prefix.to_string(), false),
                )
                .await?;
            let after = self.read_stable(keys).await?;
            if before == after {
                return Ok(StoreView {
                    values: before,
                    entries,
                });
            }
            tokio::task::yield_now().await;
        }
        Err(ActError::Store(format!(
            "sharded store view did not reach one stable version across \
             {STABLE_READ_ATTEMPTS} samples"
        )))
    }

    /// Apply the batch on its one owning shard. A batch spanning physical
    /// shards is refused: the shards share no transaction, so applying it in
    /// pieces would break the atomicity `KvStore::batch` promises.
    async fn batch(&self, ops: &[StoreBatchOp], guards: &[StoreGuard]) -> Result<bool> {
        if ops.is_empty() && guards.is_empty() {
            return Ok(true);
        }
        let index = self.inner.batch_shard(ops, guards)?;
        let _permit = self.inner.gates[index].acquire().await?;
        self.inner.shards[index].store.batch(ops, guards).await
    }

    /// Fan out to every shard and merge the ordered streams by key. The merged
    /// order is global; duplicate copies prefer the current ring owner.
    async fn scan_prefix(&self, key: &str, options: ScanOptions) -> Result<Vec<(String, Vec<u8>)>> {
        let ScanOptions {
            is_rev,
            op,
            ref prefix,
        } = options;
        let spec = ScanSpec::of(&op, key);
        let mut calls = Vec::with_capacity(self.inner.shards.len());
        for (index, shard) in self.inner.shards.iter().enumerate() {
            let store = shard.store.clone();
            let spec = spec.clone();
            let prefix = prefix.clone();
            calls.push(async move {
                let (key, options) = spec.call(&prefix, is_rev);
                store
                    .scan_prefix(&key, options)
                    .await
                    .map(|rows| (index, rows))
            });
        }

        let mut rows: Vec<(usize, String, Vec<u8>)> = Vec::new();
        for result in join_all(calls).await {
            let (index, found) = result?;
            for (key, value) in found {
                self.inner.learn_pid(&key, &value);
                rows.push((index, key, value));
            }
        }
        rows.sort_unstable_by(|left, right| left.1.cmp(&right.1));

        let mut merged: Vec<(String, Vec<u8>)> = Vec::with_capacity(rows.len());
        let mut cursor = 0;
        while cursor < rows.len() {
            let key = rows[cursor].1.clone();
            let mut end = cursor + 1;
            while end < rows.len() && rows[end].1 == key {
                end += 1;
            }
            let owner = self.inner.route_key(&key);
            let chosen = rows[cursor..end]
                .iter()
                .find(|(index, _, _)| owner == Some(*index))
                .unwrap_or(&rows[cursor]);
            merged.push((key, chosen.2.clone()));
            cursor = end;
        }
        if is_rev {
            merged.reverse();
        }
        Ok(merged)
    }
}
