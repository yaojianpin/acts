//! A bounded cache of compiled JSON Schema validators.
//!
//! Both caches in this crate that hold compiled [`Validator`]s — the runtime's
//! package-schema cache (`scheduler::validation`) and the process-wide
//! `ActSchema` cache (`model::var`) — are keyed by the schema's *serialized
//! text*, and an engine that keeps publishing workflows or packages mints a
//! new key on every schema revision: with the maps left unbounded they grow
//! with every revision the process has ever seen, not with the schemas it
//! currently runs. [`ValidatorCache`] is the bound — a capacity ceiling with
//! LRU eviction plus an idle TTL — so a long-lived engine's validator memory
//! tracks its live working set.
//!
//! The values are immutable and depend only on the schema text, so an evicted
//! entry costs a recompile the next time it is asked for and nothing else; the
//! bound is therefore safe to reach from the hot path, which is why the entry
//! is handed back as an [`Arc`] the caller keeps for as long as it needs it.

use jsonschema::Validator;
use parking_lot::Mutex;
use serde_json::Value as JsonValue;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Compiled validators a cache holds before it evicts: the same order as
/// `[data] cache_cap`'s resident-process default, so an engine keeps every
/// live package's and workflow's schema resident long before the bound is felt
/// (a deployment has one schema per package and one per workflow's
/// inputs/exposes, and only a *revision* mints a new key).
pub(crate) const DEFAULT_CAP: usize = 1024;

/// How long an entry may sit unused before it is dropped. The capacity bound
/// is what keeps the cache's footprint closed; the TTL additionally gives back
/// the validators of schemas an engine has stopped running (a package removed,
/// a workflow revision retired) without waiting for them to be pushed out.
pub(crate) const DEFAULT_TTL: Duration = Duration::from_secs(60 * 60);

/// One compiled validator, as the cache holds it.
#[derive(Debug)]
struct Entry {
    validator: Arc<Validator>,
    /// Position in [`Cache::order`], updated on every use.
    used: u64,
    /// Last insertion or use, for the idle TTL.
    at: Instant,
}

/// A capacity- and TTL-bounded map of schema text to compiled validator.
///
/// Keys are the schema's serialized text: serialization is much cheaper than
/// schema compilation, and it cannot reuse a validator for a different schema
/// the way a hash collision could.
///
/// This is a plain `Mutex`-guarded map rather than a sharded one because the
/// LRU needs a total order over uses: the map itself is only ever touched for
/// a lookup or an insertion, while the compilation that a miss leads to runs
/// outside the lock (two lanes compiling the same missing schema both do the
/// work, and the second insert simply replaces the first).
#[derive(Default)]
struct Cache {
    entries: HashMap<Arc<str>, Entry>,
    /// LRU order: every entry's `used` sequence number, lowest = least
    /// recently used. Ordered by sequence number rather than by time so
    /// eviction can take the minimum and a use can re-insert at the maximum;
    /// the sequence is assigned under the lock, so it also orders the entries
    /// by `at`.
    order: BTreeMap<u64, Arc<str>>,
    tick: u64,
}

impl Cache {
    fn get(&mut self, key: &str, ttl: Duration, now: Instant) -> Option<Arc<Validator>> {
        let (validator, used, at) = {
            let entry = self.entries.get(key)?;
            (entry.validator.clone(), entry.used, entry.at)
        };
        if now.duration_since(at) >= ttl {
            // Idle past the TTL: drop it rather than hand back an entry the
            // TTL has already retired. Its code is the caller's to recompile.
            self.entries.remove(key);
            self.order.remove(&used);
            return None;
        }
        self.touch(key, used, now);
        Some(validator)
    }

    fn insert(
        &mut self,
        key: &str,
        validator: Arc<Validator>,
        cap: usize,
        ttl: Duration,
        now: Instant,
    ) {
        self.expire(now, ttl);

        // A re-insertion (two lanes raced on a miss) is a use of the key.
        if let Some(entry) = self.entries.get_mut(key) {
            let used = entry.used;
            entry.validator = validator;
            self.touch(key, used, now);
            return;
        }

        // Make room *before* the fresh entry lands, so the cache never holds
        // more than `cap` entries.
        while self.entries.len() >= cap {
            let Some((tick, key)) = self.oldest() else {
                break;
            };
            self.order.remove(&tick);
            self.entries.remove(&*key);
        }

        self.tick += 1;
        let tick = self.tick;
        let key: Arc<str> = Arc::from(key);
        self.order.insert(tick, key.clone());
        self.entries.insert(
            key,
            Entry {
                validator,
                used: tick,
                at: now,
            },
        );
    }

    fn remove(&mut self, key: &str) {
        if let Some(entry) = self.entries.remove(key) {
            self.order.remove(&entry.used);
        }
    }

    /// Drop every entry idle past the TTL. The order map is by last use, so
    /// the entries an idle pass retires are at its front: the walk stops at
    /// the first entry still inside the TTL.
    fn expire(&mut self, now: Instant, ttl: Duration) {
        while let Some((tick, key)) = self.oldest() {
            let expired = self
                .entries
                .get(&*key)
                .is_some_and(|entry| now.duration_since(entry.at) >= ttl);
            if !expired {
                break;
            }
            self.order.remove(&tick);
            self.entries.remove(&*key);
        }
    }

    /// Mark `key` as used at `now`. `used` is the entry's current position in
    /// `order`, whose key string is reused for the new position instead of
    /// being rebuilt — a hit on the hot path allocates nothing.
    fn touch(&mut self, key: &str, used: u64, now: Instant) {
        let Some(order_key) = self.order.remove(&used) else {
            return;
        };
        self.tick += 1;
        let tick = self.tick;
        self.order.insert(tick, order_key);
        if let Some(entry) = self.entries.get_mut(key) {
            entry.used = tick;
            entry.at = now;
        }
    }

    /// The least recently used entry: its `order` position and key.
    fn oldest(&self) -> Option<(u64, Arc<str>)> {
        self.order
            .iter()
            .next()
            .map(|(tick, key)| (*tick, key.clone()))
    }
}

/// Capacity- and TTL-bounded cache of compiled JSON Schema validators, keyed
/// by the schema's serialized text. See the module docs for why it is bounded.
pub(crate) struct ValidatorCache {
    cache: Mutex<Cache>,
    cap: usize,
    ttl: Duration,
}

impl ValidatorCache {
    pub(crate) fn new() -> Self {
        Self::with_limits(DEFAULT_CAP, DEFAULT_TTL)
    }

    /// A cache holding at most `cap` validators, dropping an entry that has
    /// not been used for `ttl`. `cap` is floored at one: a cache that evicts
    /// the entry it just inserted would compile on every call instead.
    pub(crate) fn with_limits(cap: usize, ttl: Duration) -> Self {
        Self {
            cache: Mutex::new(Cache::default()),
            cap: cap.max(1),
            ttl,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.cache.lock().entries.len()
    }

    /// The compiled validator for `schema`, compiling it when the cache does
    /// not hold one. `key` is `schema`'s serialized text — the caller has it
    /// because it had to build it to ask, and a package cache keeps it to
    /// invalidate the entry with the package.
    ///
    /// `schema` is a closure because it is only needed on a miss: a hit must
    /// not pay for building the schema value the key was derived from. The
    /// compilation error is boxed — it is over 180 bytes and every caller only
    /// formats it — so the miss path's `Result` stays small.
    pub(crate) fn validator(
        &self,
        key: &str,
        schema: impl FnOnce() -> JsonValue,
    ) -> std::result::Result<Arc<Validator>, Box<jsonschema::ValidationError<'static>>> {
        if let Some(validator) = self.get(key) {
            return Ok(validator);
        }
        let validator = Arc::new(Validator::new(&schema())?);
        self.insert(key, validator.clone());
        Ok(validator)
    }

    /// The validator cached for `key`, or `None` when it is absent or has been
    /// idle past the TTL.
    pub(crate) fn get(&self, key: &str) -> Option<Arc<Validator>> {
        self.cache.lock().get(key, self.ttl, Instant::now())
    }

    /// Cache `validator` for `key`, evicting the least recently used entries
    /// beyond the capacity and any entry idle past the TTL. `key` is the
    /// serialized schema, exactly as [`Self::get`] and [`Self::validator`]
    /// spell it.
    pub(crate) fn insert(&self, key: &str, validator: Arc<Validator>) {
        let now = Instant::now();
        let mut cache = self.cache.lock();
        cache.insert(key, validator, self.cap, self.ttl, now);
    }

    /// Drop `key`'s entry, if any. Called when the schema it belongs to goes
    /// away (a package removed or republished), so its validator is released
    /// rather than left for the TTL.
    pub(crate) fn remove(&self, key: &str) {
        self.cache.lock().remove(key);
    }
}

impl Default for ValidatorCache {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for ValidatorCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValidatorCache")
            .field("len", &self.len())
            .field("cap", &self.cap)
            .field("ttl", &self.ttl)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema(field: &str) -> JsonValue {
        json!({ "type": "object", "required": [field] })
    }

    fn compile(cache: &ValidatorCache, field: &str) -> Arc<Validator> {
        let schema = schema(field);
        cache
            .validator(&schema.to_string(), || schema.clone())
            .unwrap()
    }

    #[test]
    fn validator_reuses_the_compiled_validator_for_the_same_text() {
        let cache = ValidatorCache::with_limits(8, DEFAULT_TTL);
        let schema = schema("x");
        let key = schema.to_string();
        let first = cache.validator(&key, || schema.clone()).unwrap();
        let second = cache.validator(&key, || schema.clone()).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn validator_reports_an_invalid_schema() {
        let cache = ValidatorCache::with_limits(8, DEFAULT_TTL);
        let schema = json!({ "type": "nonsense" });
        assert!(
            cache
                .validator(&schema.to_string(), || schema.clone())
                .is_err()
        );
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn capacity_evicts_the_least_recently_used() {
        let cache = ValidatorCache::with_limits(2, DEFAULT_TTL);
        compile(&cache, "a");
        compile(&cache, "b");
        // `a` was used after `b` was inserted, so `b` is now the oldest.
        compile(&cache, "a");
        compile(&cache, "c");

        assert_eq!(cache.len(), 2);
        let b = schema("b").to_string();
        assert!(cache.get(&b).is_none(), "the least recently used evicted");
        for field in ["a", "c"] {
            let key = schema(field).to_string();
            assert!(cache.get(&key).is_some(), "{field} kept");
        }
    }

    #[test]
    fn expired_entries_are_dropped_and_recompiled() {
        let ttl = Duration::from_millis(20);
        let cache = ValidatorCache::with_limits(8, ttl);
        let first = compile(&cache, "a");
        assert_eq!(cache.len(), 1);

        std::thread::sleep(Duration::from_millis(50));
        assert!(cache.get(&schema("a").to_string()).is_none());
        assert_eq!(cache.len(), 0, "an idle lookup retires the entry");

        let second = compile(&cache, "a");
        assert!(
            !Arc::ptr_eq(&first, &second),
            "the expired one was recompiled"
        );
    }

    #[test]
    fn insert_expires_idle_entries_of_other_keys() {
        let ttl = Duration::from_millis(20);
        let cache = ValidatorCache::with_limits(8, ttl);
        compile(&cache, "a");

        std::thread::sleep(Duration::from_millis(50));
        compile(&cache, "b");

        assert_eq!(cache.len(), 1, "the idle `a` is gone");
        assert!(cache.get(&schema("b").to_string()).is_some());
    }

    #[test]
    fn remove_drops_the_entry() {
        let cache = ValidatorCache::with_limits(8, DEFAULT_TTL);
        compile(&cache, "a");
        cache.remove(&schema("a").to_string());
        assert_eq!(cache.len(), 0);
        assert!(cache.get(&schema("a").to_string()).is_none());
    }

    #[test]
    fn capacity_is_floored_at_one() {
        let cache = ValidatorCache::with_limits(0, DEFAULT_TTL);
        compile(&cache, "a");
        compile(&cache, "b");
        assert_eq!(cache.len(), 1);
    }
}
