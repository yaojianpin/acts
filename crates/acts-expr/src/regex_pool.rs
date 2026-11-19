//! Constant and dynamic regular expressions with a bounded cache.
//!
//! `is_match` is the one built-in that can compile user data. A pattern that
//! is a string literal is compiled once when the expression is compiled, so a
//! task that matches the same pattern a million times compiles it zero times.
//! A dynamic pattern is compiled into a small LRU: entries and bytes are
//! capped, so a workflow cannot make the process remember arbitrary patterns.
//! Patterns and inputs are length-bounded before compilation/matching, and an
//! error carries a short escaped preview and a hash, never the full pattern.

#[cfg(feature = "regex")]
use crate::eval::EvalBudget;
#[cfg(feature = "regex")]
use crate::parser::Node;
#[cfg(feature = "regex")]
use crate::{CompileLimits, Error, Result};

#[cfg(feature = "regex")]
use regex::Regex;
#[cfg(feature = "regex")]
use std::collections::{HashMap, VecDeque};
#[cfg(feature = "regex")]
use std::sync::{Arc, Mutex};

#[cfg(feature = "regex")]
const DEFAULT_MAX_INPUT: usize = 1024 * 1024;
#[cfg(feature = "regex")]
const DEFAULT_MAX_ENTRIES: usize = 128;
#[cfg(feature = "regex")]
const DEFAULT_MAX_BYTES: usize = 16 * 1024 * 1024;

/// The expression's regex state.
#[cfg(feature = "regex")]
pub(crate) struct RegexPool {
    constants: HashMap<Arc<str>, Arc<Regex>>,
    dynamic: Mutex<DynamicCache>,
    max_pattern: usize,
    max_input: usize,
    max_entries: usize,
    max_bytes: usize,
}

#[cfg(feature = "regex")]
#[derive(Default)]
struct DynamicCache {
    map: HashMap<Arc<str>, Arc<Regex>>,
    order: VecDeque<Arc<str>>,
    bytes: usize,
    evictions: u64,
}

#[cfg(feature = "regex")]
impl Default for RegexPool {
    fn default() -> RegexPool {
        RegexPool {
            constants: HashMap::new(),
            dynamic: Mutex::new(DynamicCache::default()),
            max_pattern: 8 * 1024,
            max_input: DEFAULT_MAX_INPUT,
            max_entries: DEFAULT_MAX_ENTRIES,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }
}

#[cfg(feature = "regex")]
impl RegexPool {
    /// Compile every literal `is_match('...')` pattern once. A literal that
    /// is not a valid regex fails the expression compilation.
    pub(crate) fn compile_constants(nodes: &[Node], limits: &CompileLimits) -> Result<RegexPool> {
        let mut constants = HashMap::new();

        for node in nodes {
            let Node::Method(_, name, args) = node else {
                continue;
            };
            if name.as_ref() != "is_match" || args.len() != 1 {
                continue;
            }
            let Some(Node::Str(pattern)) = nodes.get(args[0] as usize) else {
                continue;
            };
            if pattern.len() > limits.regex_pattern_bytes {
                return Err(Error::TooLarge {
                    what: "regular expression pattern",
                    limit: limits.regex_pattern_bytes,
                });
            }
            let regex = Regex::new(pattern).map_err(|error| regex_error(pattern, error))?;
            constants.insert(Arc::clone(pattern), Arc::new(regex));
        }

        Ok(RegexPool {
            constants,
            dynamic: Mutex::new(DynamicCache::default()),
            max_pattern: limits.regex_pattern_bytes,
            max_input: DEFAULT_MAX_INPUT,
            max_entries: DEFAULT_MAX_ENTRIES,
            max_bytes: DEFAULT_MAX_BYTES,
        })
    }

    /// Bound the input and charge the match cost.
    pub(crate) fn check_input(&self, text: &str, budget: &mut EvalBudget) -> Result<()> {
        if text.len() > self.max_input {
            return Err(Error::TooLarge {
                what: "regular expression input",
                limit: self.max_input,
            });
        }
        budget.charge_steps_for_match(text.len())
    }

    /// The compiled pattern: a constant from compile time, a dynamic cache
    /// hit, or a fresh compile that is inserted only while it fits.
    pub(crate) fn get(&self, pattern: &str, budget: &mut EvalBudget) -> Result<Arc<Regex>> {
        if pattern.len() > self.max_pattern {
            return Err(Error::TooLarge {
                what: "regular expression pattern",
                limit: self.max_pattern,
            });
        }
        if let Some(regex) = self.constants.get(pattern) {
            return Ok(Arc::clone(regex));
        }

        {
            let mut cache = self.lock();
            if let Some(regex) = cache.map.get(pattern).cloned() {
                cache.touch(pattern);
                return Ok(regex);
            }
        }

        budget.charge_steps_for_regex_compile(pattern.len())?;
        let regex = Arc::new(Regex::new(pattern).map_err(|error| regex_error(pattern, error))?);
        let bytes = estimate(pattern);

        if bytes <= self.max_bytes {
            let mut cache = self.lock();
            if !cache.map.contains_key(pattern) {
                cache.map.insert(Arc::from(pattern), Arc::clone(&regex));
                cache.order.push_back(Arc::from(pattern));
                cache.bytes = cache.bytes.saturating_add(bytes);
                cache.evict(self.max_entries, self.max_bytes);
            }
        }

        Ok(regex)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, DynamicCache> {
        self.dynamic
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    #[cfg(test)]
    pub(crate) fn new() -> RegexPool {
        RegexPool::default()
    }

    #[cfg(test)]
    pub(crate) fn cache_stats(&self) -> (usize, usize, u64) {
        let cache = self.lock();
        (cache.map.len(), cache.bytes, cache.evictions)
    }
}

#[cfg(all(test, feature = "regex"))]
mod tests {
    use super::*;
    use crate::BudgetProfile;

    #[test]
    fn a_dynamic_cache_is_bounded_and_evicts_least_recently_used() {
        let pool = RegexPool::default();
        let mut budget = EvalBudget::for_profile(BudgetProfile::Task);

        for index in 0..(DEFAULT_MAX_ENTRIES + 32) {
            pool.get(&format!("^value-{index}$"), &mut budget).unwrap();
        }

        let (entries, bytes, evictions) = pool.cache_stats();
        assert!(entries <= DEFAULT_MAX_ENTRIES, "{entries}");
        assert!(bytes <= DEFAULT_MAX_BYTES, "{bytes}");
        assert!(evictions >= 32, "{evictions}");
    }

    #[test]
    fn a_long_or_invalid_pattern_is_bounded_and_sanitized() {
        let pool = RegexPool::default();
        let mut budget = EvalBudget::for_profile(BudgetProfile::Task);

        let long = "a".repeat(DEFAULT_MAX_PATTERN_SENTINEL);
        assert!(matches!(
            pool.get(&long, &mut budget),
            Err(Error::TooLarge { .. })
        ));

        let pattern = format!("{}(", "a".repeat(200));
        let error = pool.get(&pattern, &mut budget).unwrap_err().to_string();
        assert!(error.contains("regular expression"), "{error}");
        assert!(!error.contains(&"a".repeat(200)), "{error}");
        assert!(error.contains('…'), "{error}");
    }

    const DEFAULT_MAX_PATTERN_SENTINEL: usize = 8 * 1024 + 1;
}

#[cfg(feature = "regex")]
impl DynamicCache {
    fn touch(&mut self, pattern: &str) {
        if let Some(position) = self.order.iter().position(|key| key.as_ref() == pattern)
            && let Some(key) = self.order.remove(position)
        {
            self.order.push_back(key);
        }
    }

    fn evict(&mut self, max_entries: usize, max_bytes: usize) {
        while self.map.len() > max_entries || self.bytes > max_bytes {
            let Some(key) = self.order.pop_front() else {
                break;
            };
            if self.map.remove(&key).is_some() {
                self.bytes = self.bytes.saturating_sub(estimate(&key));
                self.evictions = self.evictions.saturating_add(1);
            }
        }
    }
}

#[cfg(feature = "regex")]
fn estimate(pattern: &str) -> usize {
    pattern.len().saturating_mul(4).saturating_add(4096)
}

#[cfg(feature = "regex")]
fn regex_error(pattern: &str, _error: regex::Error) -> Error {
    let mut preview = String::new();
    let mut chars = pattern.chars();
    for _ in 0..128 {
        let Some(c) = chars.next() else {
            break;
        };
        preview.extend(c.escape_default());
    }
    if chars.next().is_some() {
        preview.push('…');
    }

    Error::Function {
        name: "is_match".to_string(),
        message: format!(
            "'{preview}' is not a regular expression (length {}, hash {:016x})",
            pattern.len(),
            hash(pattern)
        ),
    }
}

#[cfg(feature = "regex")]
fn hash(text: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Without the `regex` feature there is no pattern to compile; the type keeps
/// the expression and evaluator shapes independent of the feature.
#[cfg(not(feature = "regex"))]
#[derive(Default)]
pub(crate) struct RegexPool;

#[cfg(not(feature = "regex"))]
impl RegexPool {
    pub(crate) fn compile_constants(
        _nodes: &[crate::parser::Node],
        _limits: &crate::CompileLimits,
    ) -> crate::Result<RegexPool> {
        Ok(RegexPool)
    }

    #[cfg(test)]
    pub(crate) fn new() -> RegexPool {
        RegexPool
    }
}

#[cfg(all(test, feature = "regex"))]
mod coverage_tests {
    use super::DEFAULT_MAX_INPUT;
    use super::*;
    use crate::{BudgetProfile, CompileLimits, Context, Error, Expr};

    #[test]
    fn compile_skips_non_literal_patterns_and_checks_limits() {
        for source in [
            "1 + 2",
            "'x'.length()",
            "'x'.is_match(p)",
            "'x'.is_match('a', 'b')",
        ] {
            assert!(Expr::compile(source).is_ok(), "{source}");
        }

        let limits = CompileLimits {
            regex_pattern_bytes: 2,
            ..CompileLimits::default()
        };
        assert!(matches!(
            Expr::compile_with("'x'.is_match('abcd')", &limits),
            Err(Error::TooLarge {
                what: "regular expression pattern",
                limit: 2
            })
        ));
    }

    #[test]
    fn a_constant_pattern_is_compiled_once_and_used() {
        let mut context = Context::new();
        context.set("text", "abc");
        let expr = Expr::compile("text.is_match('^a.c$')").unwrap();
        assert_eq!(expr.eval(&context).unwrap(), crate::Value::Bool(true));
        assert_eq!(expr.eval(&context).unwrap(), crate::Value::Bool(true));
    }

    #[test]
    fn dynamic_hits_and_oversized_input_are_bounded() {
        let pool = RegexPool::default();
        let mut budget = EvalBudget::for_profile(BudgetProfile::Task);

        assert!(pool.get("^a$", &mut budget).is_ok());
        assert!(pool.get("^a$", &mut budget).is_ok());
        let (entries, _, _) = pool.cache_stats();
        assert_eq!(entries, 1);

        let oversized = "x".repeat(DEFAULT_MAX_INPUT + 1);
        assert!(matches!(
            pool.check_input(&oversized, &mut budget),
            Err(Error::TooLarge {
                what: "regular expression input",
                ..
            })
        ));
    }
}
