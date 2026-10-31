//! The join rule of a parallel container.
//!
//! A container that fans parallel children out (a step's branches, or the
//! virtual branches a `acts.core.block{ mode: parallel }`/`acts.core.parallel`
//! step materializes) parks for them and decides by one [`JoinRule`]:
//!
//! - [`JoinRule::All`] (the default, and the shape before rules existed):
//!   every child settles, none aborted, then the container completes — a child
//!   failure still fails the whole ancestor chain through the error walk.
//! - [`JoinRule::Any`]: the first completion decides.
//! - [`JoinRule::Partial`]: a count or a percentage of completions decides
//!   (`partial(3)`, `partial(40%)`). `partial(1)` is `any`,
//!   `partial(100%)` is `all`.
//! - [`JoinRule::Action`]: the rule is delegated — on every child settlement
//!   the engine asks the package the rule names ([`DECIDE_PACKAGE`] by
//!   default) and the answer's verdict (complete/fail) decides the container;
//!   a skipped request defers.
//!
//! The declaration is one field with a string or an object shape:
//!
//! ```yaml
//! rule: all                  # the default
//! rule: any
//! rule: partial(3)           # count
//! rule: partial(40%)         # percentage
//! rule:                      # delegated to a package
//!   params:                  # free-form, resolved against the container scope
//!     action: countersign
//!   uses: acts.core.decide   # the deciding package; this is the default
//! ```

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// The core package that answers a delegated join rule — [`JoinRule::Action`]'s
/// default `uses`. A model may name its own deciding package instead; the
/// kernel only defines the request envelope (`params` + the children's
/// outcomes) and the verdict vocabulary.
pub const DECIDE_PACKAGE: &str = "acts.core.decide";

/// The number of counted children a rule needs before it decides.
///
/// A *counted* child is one that was really in the vote: a branch its own
/// guard skipped (`Skipped`) or one the client removed (`Removed`) was never
/// part of it. A completion is a child that answered successfully
/// (`Completed`/`Submitted`); a settled non-completion (`Failed`, `Cancelled`,
/// `Backed`) can never pass, so it shrinks what the remaining children can
/// still reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartialThreshold {
    /// `partial(n)` — a fixed number of completions decides.
    Count(u32),
    /// `partial(p%)` — `ceil(p% × N)` completions decide, `N` the counted
    /// children. The count is only known once the container fans out, so the
    /// threshold is computed at join time.
    Percent(u32),
}

impl PartialThreshold {
    /// The wire form inside `partial(...)`: a bare count or a `%` percentage.
    fn text(&self) -> String {
        match self {
            Self::Count(count) => count.to_string(),
            Self::Percent(percent) => format!("{percent}%"),
        }
    }

    fn parse(text: &str) -> Result<Self, String> {
        let bad = || {
            format!(
                "invalid join 'partial({text})': a count of 1 or more, or a percentage of 1%-100%"
            )
        };
        if let Some(percent) = text.strip_suffix('%') {
            let percent: u32 = percent.parse().map_err(|_| bad())?;
            if !(1..=100).contains(&percent) {
                return Err(bad());
            }
            return Ok(Self::Percent(percent));
        }
        let count: u32 = text.parse().map_err(|_| bad())?;
        if count == 0 {
            return Err(bad());
        }
        Ok(Self::Count(count))
    }
}

/// The join rule of one parallel container (see the [module
/// documentation](self)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JoinRule {
    /// Every counted child must complete; a child failure fails the whole
    /// ancestor chain (the error walk), exactly as the engine always behaved.
    All,
    /// The first completion decides the container.
    Any,
    /// A threshold of completions decides the container.
    Partial(PartialThreshold),
    /// The rule is delegated: on every child settlement the engine asks the
    /// package `uses` names — the request carries the rule's `params` and the
    /// children's outcomes census — answered `complete` (pass), `fail`
    /// (reject) or `skip` (defer).
    Action {
        /// Free-form parameters the request carries, resolved against the
        /// container's scope (`${{ }}` templates included). The deciding
        /// package gives them their meaning; the default
        /// [`DECIDE_PACKAGE`] reads an `action` key naming the client's
        /// business handler.
        params: serde_json::Value,
        /// The package that decides. Defaults to [`DECIDE_PACKAGE`]; a model
        /// names its own extension package here.
        uses: String,
    },
}

impl JoinRule {
    /// Parse the string form: `all`, `any`, `partial(n)`, `partial(p%)`.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        match text.trim() {
            "all" => Ok(Self::All),
            "any" => Ok(Self::Any),
            other => {
                let bad = || {
                    format!(
                        "invalid join '{other}': expected 'all', 'any', 'partial(n)' or 'partial(n%)'"
                    )
                };
                let inner = other
                    .strip_prefix("partial(")
                    .and_then(|rest| rest.strip_suffix(')'))
                    .ok_or_else(bad)?;
                if inner.is_empty() {
                    return Err(bad());
                }
                Ok(Self::Partial(PartialThreshold::parse(inner)?))
            }
        }
    }

    /// Whether a child's failure is a vote against the container instead of
    /// the container's own death: the threshold rules and the delegated one
    /// can still decide after a failure, `all` cannot (the error walk keeps
    /// its legacy shape).
    pub(crate) fn tolerates_failure(&self) -> bool {
        !matches!(self, Self::All)
    }

    /// The completions the rule waits for over `total` counted children.
    ///
    /// `partial(100%)` is `all` and `partial(1)` is `any` by the arithmetic
    /// alone. A count above `total` is returned as declared — the join reads
    /// it as *unreachable* (the container fails), and the fan-out refuses the
    /// declaration loudly the moment the real children are known.
    pub(crate) fn threshold(&self, total: usize) -> usize {
        match self {
            Self::All => total,
            Self::Any => 1,
            Self::Partial(PartialThreshold::Count(count)) => *count as usize,
            Self::Partial(PartialThreshold::Percent(percent)) => {
                (total * *percent as usize).div_ceil(100)
            }
            // A delegated container decides by verdict, not by count; the
            // join never asks for its threshold.
            Self::Action { .. } => total,
        }
    }
}

impl fmt::Display for JoinRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::All => f.write_str("all"),
            Self::Any => f.write_str("any"),
            Self::Partial(threshold) => write!(f, "partial({})", threshold.text()),
            Self::Action { uses, .. } => write!(f, "action({uses})"),
        }
    }
}

impl Serialize for JoinRule {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::All => serializer.serialize_str("all"),
            Self::Any => serializer.serialize_str("any"),
            Self::Partial(threshold) => {
                serializer.serialize_str(&format!("partial({})", threshold.text()))
            }
            Self::Action { params, uses } => {
                use serde::ser::SerializeStruct;
                let mut map = serializer.serialize_struct("JoinRule", 2)?;
                map.serialize_field("params", params)?;
                map.serialize_field("uses", uses)?;
                map.end()
            }
        }
    }
}

/// The serde visitor: a string rule or a `{ params, uses }` object.
struct JoinRuleVisitor;

impl<'de> serde::de::Visitor<'de> for JoinRuleVisitor {
    type Value = JoinRule;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(
            "a join rule: 'all', 'any', 'partial(n)', 'partial(n%)' or an object with 'params' and 'uses' keys",
        )
    }

    fn visit_str<E>(self, text: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        JoinRule::parse(text).map_err(E::custom)
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        use serde::de::Error;
        let mut uses: Option<String> = None;
        let mut params = serde_json::Value::Null;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "uses" => uses = Some(map.next_value()?),
                "params" => params = map.next_value()?,
                other => {
                    return Err(A::Error::custom(format!(
                        "unknown rule field '{other}': expected 'uses' or 'params'"
                    )));
                }
            }
        }
        let uses = uses
            .map(|uses| uses.trim().to_string())
            .filter(|uses| !uses.is_empty())
            .unwrap_or_else(|| crate::model::join::DECIDE_PACKAGE.to_string());
        Ok(JoinRule::Action { params, uses })
    }
}

impl<'de> Deserialize<'de> for JoinRule {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(JoinRuleVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(text: &str) -> String {
        JoinRule::parse(text).unwrap_err()
    }

    #[test]
    fn parses_the_string_forms() {
        assert_eq!(JoinRule::parse("all").unwrap(), JoinRule::All);
        assert_eq!(JoinRule::parse("any").unwrap(), JoinRule::Any);
        assert_eq!(
            JoinRule::parse("partial(3)").unwrap(),
            JoinRule::Partial(PartialThreshold::Count(3))
        );
        assert_eq!(
            JoinRule::parse("partial(40%)").unwrap(),
            JoinRule::Partial(PartialThreshold::Percent(40))
        );
        // whitespace around the word is the YAML author's, not part of it
        assert_eq!(JoinRule::parse(" any ").unwrap(), JoinRule::Any);
    }

    #[test]
    fn refuses_malformed_or_impossible_thresholds() {
        for text in [
            "partially(3)",
            "partial",
            "partial()",
            "partial(0)",
            "partial(0%)",
            "partial(101%)",
            "partial(-1)",
            "partial(1.5)",
            "partial(3 %)",
            "sometimes",
            "",
        ] {
            assert!(!err(text).is_empty(), "{text} must be refused");
        }
    }

    #[test]
    fn round_trips_through_serde() {
        for rule in [
            JoinRule::All,
            JoinRule::Any,
            JoinRule::Partial(PartialThreshold::Count(3)),
            JoinRule::Partial(PartialThreshold::Percent(40)),
            JoinRule::Action {
                params: serde_json::json!({ "chair": "chair" }),
                uses: DECIDE_PACKAGE.to_string(),
            },
        ] {
            let text = serde_json::to_string(&rule).unwrap();
            assert_eq!(serde_json::from_str::<JoinRule>(&text).unwrap(), rule);
        }
    }

    #[test]
    fn serializes_the_canonical_forms() {
        assert_eq!(
            serde_json::to_value(JoinRule::Partial(PartialThreshold::Percent(40))).unwrap(),
            serde_json::json!("partial(40%)")
        );
        assert_eq!(
            serde_json::to_value(JoinRule::Action {
                params: serde_json::json!({ "action": "sign" }),
                uses: DECIDE_PACKAGE.to_string(),
            })
            .unwrap(),
            serde_json::json!({ "params": { "action": "sign" }, "uses": DECIDE_PACKAGE })
        );
    }

    #[test]
    fn refuses_unknown_or_empty_action_objects() {
        let bad: Result<JoinRule, _> = serde_json::from_value(serde_json::json!({ "key": "sign" }));
        assert!(bad.unwrap_err().to_string().contains("unknown rule field"));
        // An object without 'uses' falls back to the core deciding package.
        let empty: JoinRule = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(
            empty,
            JoinRule::Action {
                params: serde_json::Value::Null,
                uses: DECIDE_PACKAGE.to_string()
            }
        );
    }

    #[test]
    fn thresholds_follow_the_arithmetic() {
        let count = |n: u32| JoinRule::Partial(PartialThreshold::Count(n));
        let percent = |p: u32| JoinRule::Partial(PartialThreshold::Percent(p));
        assert_eq!(JoinRule::All.threshold(5), 5);
        assert_eq!(JoinRule::Any.threshold(5), 1);
        assert_eq!(count(3).threshold(5), 3);
        assert_eq!(percent(40).threshold(5), 2, "ceil(5 × 40%)");
        assert_eq!(percent(50).threshold(5), 3, "ceil(2.5) rounds up");
        assert_eq!(percent(100).threshold(5), 5, "partial(100%) is all");
        assert_eq!(count(1).threshold(5), 1, "partial(1) is any");
        assert_eq!(percent(40).threshold(1), 1, "one child rounds to one");
        assert_eq!(
            count(5).threshold(3),
            5,
            "a count above the children is unreachable, not clamped"
        );
    }

    #[test]
    fn only_all_keeps_the_legacy_failure_walk() {
        assert!(!JoinRule::All.tolerates_failure());
        assert!(JoinRule::Any.tolerates_failure());
        assert!(JoinRule::Partial(PartialThreshold::Count(2)).tolerates_failure(),);
        assert!(
            JoinRule::Action {
                params: serde_json::Value::Null,
                uses: DECIDE_PACKAGE.to_string(),
            }
            .tolerates_failure()
        );
    }

    #[test]
    fn renders_for_errors_and_messages() {
        assert_eq!(JoinRule::All.to_string(), "all");
        assert_eq!(
            JoinRule::Partial(PartialThreshold::Percent(40)).to_string(),
            "partial(40%)"
        );
        assert_eq!(
            JoinRule::Action {
                params: serde_json::Value::Null,
                uses: "acme.countersign".to_string(),
            }
            .to_string(),
            "action(acme.countersign)"
        );
    }
}
