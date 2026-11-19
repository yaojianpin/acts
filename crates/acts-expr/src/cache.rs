//! A bounded cache of compiled expressions.
//!
//! A cache key is not the source hash alone: the same source can mean
//! something else after a parser semantics change, a function registry change,
//! a different expected result type, a different budget profile or different
//! compile limits. The entry also clones the source and compares it, so a hash
//! collision can never serve the wrong program.

use crate::parser::Node;
use crate::{BudgetProfile, CompileLimits, Expr, ExprType, SEMANTICS_VERSION};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

/// Everything that can change the meaning or acceptance of a compiled source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub source_hash: u64,
    pub semantics_version: u32,
    pub function_generation: u64,
    pub expected_type: ExprType,
    pub profile: BudgetProfile,
    pub limits: CompileLimits,
}

impl CacheKey {
    pub fn new(
        source: &str,
        function_generation: u64,
        expected_type: ExprType,
        profile: BudgetProfile,
    ) -> CacheKey {
        CacheKey {
            source_hash: crate::hash_source(source),
            semantics_version: SEMANTICS_VERSION,
            function_generation,
            expected_type,
            profile,
            limits: profile.compile_limits(),
        }
    }

    /// Use explicit compile limits instead of the profile's.
    pub fn with_limits(mut self, limits: CompileLimits) -> CacheKey {
        self.limits = limits;
        self
    }
}

/// A snapshot of the cache's bounded state, for status and metrics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub entries: usize,
    pub bytes: usize,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

struct Entry {
    source: Arc<str>,
    expr: Expr,
    bytes: usize,
}

struct Inner {
    entries: HashMap<CacheKey, Entry>,
    order: VecDeque<CacheKey>,
    bytes: usize,
    hits: u64,
    misses: u64,
    evictions: u64,
}

impl Inner {
    fn new() -> Inner {
        Inner {
            entries: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            hits: 0,
            misses: 0,
            evictions: 0,
        }
    }

    fn touch(&mut self, key: &CacheKey) {
        if let Some(position) = self.order.iter().position(|item| item == key)
            && let Some(item) = self.order.remove(position)
        {
            self.order.push_back(item);
        }
    }

    fn evict(&mut self, max_entries: usize, max_bytes: usize) {
        while self.entries.len() > max_entries || self.bytes > max_bytes {
            let Some(key) = self.order.pop_front() else {
                break;
            };
            if let Some(entry) = self.entries.remove(&key) {
                self.bytes = self.bytes.saturating_sub(entry.bytes);
                self.evictions = self.evictions.saturating_add(1);
            }
        }
    }
}

/// A thread-safe, bounded cache of compiled expressions.
///
/// `max_entries` and `max_bytes` are hard caps; an expression larger than the
/// whole cache is returned to the caller without being stored. Entries are
/// evicted least-recently-used first.
pub struct ExprCache {
    max_entries: usize,
    max_bytes: usize,
    inner: Mutex<Inner>,
}

impl Default for ExprCache {
    fn default() -> ExprCache {
        ExprCache::new(1024, 4 * 1024 * 1024)
    }
}

impl ExprCache {
    pub fn new(max_entries: usize, max_bytes: usize) -> ExprCache {
        ExprCache {
            max_entries,
            max_bytes,
            inner: Mutex::new(Inner::new()),
        }
    }

    /// Return the cached expression for `key`, or compile it with the key's
    /// profile and limits.
    pub fn get_or_compile(&self, source: &str, key: &CacheKey) -> crate::Result<Expr> {
        {
            let mut inner = self.lock();
            if let Some(entry) = inner.entries.get(key)
                && entry.source.as_ref() == source
            {
                let expr = entry.expr.clone();
                inner.hits = inner.hits.saturating_add(1);
                inner.touch(key);
                return Ok(expr);
            }
            inner.misses = inner.misses.saturating_add(1);
        }

        let expr = Expr::compile_typed_full(source, key.expected_type, key.profile, &key.limits)?;
        let source: Arc<str> = Arc::from(source);
        let bytes = source
            .len()
            .saturating_add(expr.len().saturating_mul(std::mem::size_of::<Node>()))
            .saturating_add(128);

        if bytes <= self.max_bytes && self.max_entries > 0 {
            let mut inner = self.lock();
            if let Some(previous) = inner.entries.remove(key) {
                inner.bytes = inner.bytes.saturating_sub(previous.bytes);
            }
            if let Some(position) = inner.order.iter().position(|item| item == key) {
                inner.order.remove(position);
            }

            inner.bytes = inner.bytes.saturating_add(bytes);
            inner.entries.insert(
                *key,
                Entry {
                    source,
                    expr: expr.clone(),
                    bytes,
                },
            );
            inner.order.push_back(*key);
            inner.evict(self.max_entries, self.max_bytes);
        }

        Ok(expr)
    }

    pub fn stats(&self) -> CacheStats {
        let inner = self.lock();
        CacheStats {
            entries: inner.entries.len(),
            bytes: inner.bytes,
            hits: inner.hits,
            misses: inner.misses,
            evictions: inner.evictions,
        }
    }

    pub fn clear(&self) {
        let mut inner = self.lock();
        inner.entries.clear();
        inner.order.clear();
        inner.bytes = 0;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_key_compiles_once_and_a_generation_is_a_new_entry() {
        let cache = ExprCache::new(8, 1024 * 1024);
        let source = "a + 1";
        let key = CacheKey::new(source, 1, ExprType::Any, BudgetProfile::Task);

        let first = cache.get_or_compile(source, &key).unwrap();
        let second = cache.get_or_compile(source, &key).unwrap();
        assert_eq!(first.source_hash(), second.source_hash());
        assert_eq!(cache.stats().entries, 1);
        assert_eq!(cache.stats().hits, 1);

        let other = CacheKey::new(source, 2, ExprType::Any, BudgetProfile::Task);
        cache.get_or_compile(source, &other).unwrap();
        assert_eq!(cache.stats().entries, 2);
    }

    #[test]
    fn entries_and_bytes_are_bounded() {
        let cache = ExprCache::new(2, 1024 * 1024);
        for source in ["1", "2", "3"] {
            let key = CacheKey::new(source, 0, ExprType::Any, BudgetProfile::Task);
            cache.get_or_compile(source, &key).unwrap();
        }

        let stats = cache.stats();
        assert_eq!(stats.entries, 2);
        assert!(stats.evictions >= 1, "{stats:?}");
        assert!(stats.bytes <= 1024 * 1024, "{stats:?}");
    }

    #[test]
    fn a_source_hash_collision_does_not_serve_the_wrong_program() {
        let cache = ExprCache::new(8, 1024 * 1024);
        let first = CacheKey::new("1 + 1", 0, ExprType::Any, BudgetProfile::Task);
        let second = CacheKey::new("2 + 2", 0, ExprType::Any, BudgetProfile::Task);
        // Force the same hash to exercise the source comparison.
        let forged = CacheKey {
            source_hash: first.source_hash,
            ..second
        };

        let context = crate::Context::new();
        let one = cache.get_or_compile("1 + 1", &first).unwrap();
        let two = cache.get_or_compile("2 + 2", &forged).unwrap();
        assert_eq!(one.eval(&context).unwrap(), crate::Value::Int(2));
        assert_eq!(two.eval(&context).unwrap(), crate::Value::Int(4));
    }
}

#[cfg(test)]
mod coverage_tests {
    use super::*;

    #[test]
    fn cache_keys_can_override_limits_and_regions_can_clear() {
        let limits = CompileLimits {
            nodes: 64,
            ..CompileLimits::default()
        };
        let key = CacheKey::new("1 + 2", 0, ExprType::Any, BudgetProfile::Task).with_limits(limits);
        assert_eq!(key.limits.nodes, 64);

        let cache = ExprCache::default();
        assert_eq!(cache.stats(), CacheStats::default());
        cache.get_or_compile("1 + 2", &key).unwrap();
        assert!(cache.stats().entries > 0);
        cache.clear();
        assert_eq!(cache.stats().entries, 0);
        assert_eq!(cache.stats().bytes, 0);
    }

    #[test]
    fn an_oversized_or_invalid_expression_is_not_cached() {
        let cache = ExprCache::new(8, 16);
        let key = CacheKey::new("1 + 2", 0, ExprType::Any, BudgetProfile::Task);
        cache.get_or_compile("1 + 2", &key).unwrap();
        assert_eq!(cache.stats().entries, 0);

        let cache = ExprCache::new(8, 1024 * 1024);
        let key = CacheKey::new("1 +", 0, ExprType::Any, BudgetProfile::Task);
        assert!(cache.get_or_compile("1 +", &key).is_err());
        assert_eq!(cache.stats().entries, 0);
        assert_eq!(cache.stats().misses, 1);

        let cache = ExprCache::new(0, 1024 * 1024);
        let key = CacheKey::new("1", 0, ExprType::Any, BudgetProfile::Task);
        cache.get_or_compile("1", &key).unwrap();
        assert_eq!(cache.stats().entries, 0);
    }

    #[test]
    fn a_cache_hit_is_refreshed_and_misses_are_counted() {
        let cache = ExprCache::new(2, 1024 * 1024);
        let one = CacheKey::new("1", 0, ExprType::Any, BudgetProfile::Task);
        let two = CacheKey::new("2", 0, ExprType::Any, BudgetProfile::Task);

        cache.get_or_compile("1", &one).unwrap();
        cache.get_or_compile("2", &two).unwrap();
        cache.get_or_compile("1", &one).unwrap();
        cache
            .get_or_compile(
                "3",
                &CacheKey::new("3", 0, ExprType::Any, BudgetProfile::Task),
            )
            .unwrap();

        let stats = cache.stats();
        assert_eq!(stats.entries, 2);
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 3);
        assert!(stats.evictions >= 1);
    }
}
