//! A step's custom input validations.
//!
//! `rules` on a step is a list of assertions over the step's resolved params
//! and its scope: every expression must hold when the act starts, before the
//! act announces itself to a client or runs its package. One that does not
//! hold fails the step with the `validation_failed` error and the rule's own
//! message — the standard error walk takes over, so a `catches` handler can
//! pick the failure up like any other.
//!
//! ```yaml
//! - id: review
//!   uses: acts.core.irq
//!   vars:
//!     - { name: amount, value: 0 }
//!   params:
//!     key: review
//!   rules:
//!     - if: "amount > 0"        # must hold; else the step fails
//!       message: amount must be positive
//!     - "count <= 100"          # shorthand: no custom message
//! ```

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// One input validation of a step: an expression that must hold when the act
/// starts (see the [module documentation](self)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationRule {
    /// The expression, decided against the step's scope with its resolved
    /// params on top — the same expression language a branch guard speaks.
    pub r#if: String,
    /// The failure message; the expression itself when absent.
    pub message: Option<String>,
}

impl ValidationRule {
    /// Parse the object or string form. A string is the expression without a
    /// custom message.
    pub(crate) fn parse(if_expr: &str, message: Option<String>) -> Result<Self, String> {
        let expr = if_expr.trim();
        if expr.is_empty() {
            return Err("a validation rule needs a non-empty expression".to_string());
        }
        Ok(Self {
            r#if: expr.to_string(),
            message,
        })
    }

    /// The message a violated rule fails with.
    pub(crate) fn failure_message(&self) -> String {
        self.message
            .clone()
            .unwrap_or_else(|| format!("validation failed: {}", self.r#if))
    }
}

impl Serialize for ValidationRule {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match &self.message {
            None => serializer.serialize_str(&self.r#if),
            Some(message) => {
                use serde::ser::SerializeStruct;
                let mut map = serializer.serialize_struct("ValidationRule", 2)?;
                map.serialize_field("if", &self.r#if)?;
                map.serialize_field("message", message)?;
                map.end()
            }
        }
    }
}

/// The serde visitor: a bare expression or an `{ if, message }` object.
struct ValidationRuleVisitor;

impl<'de> serde::de::Visitor<'de> for ValidationRuleVisitor {
    type Value = ValidationRule;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(
            "a validation rule: an expression, or an object with an 'if' and an optional 'message'",
        )
    }

    fn visit_str<E>(self, text: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        ValidationRule::parse(text, None).map_err(E::custom)
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        use serde::de::Error;
        let mut if_expr: Option<String> = None;
        let mut message: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "if" => if_expr = Some(map.next_value()?),
                "message" => message = Some(map.next_value()?),
                other => {
                    return Err(A::Error::custom(format!(
                        "unknown validation field '{other}': expected 'if' or 'message'"
                    )));
                }
            }
        }
        let if_expr = if_expr.ok_or_else(|| A::Error::custom("a validation rule needs an 'if'"))?;
        ValidationRule::parse(&if_expr, message).map_err(A::Error::custom)
    }
}

impl<'de> Deserialize<'de> for ValidationRule {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(ValidationRuleVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_object_and_string_forms() {
        assert_eq!(
            serde_json::from_value::<ValidationRule>(serde_json::json!("amount > 0")).unwrap(),
            ValidationRule {
                r#if: "amount > 0".into(),
                message: None,
            }
        );
        assert_eq!(
            serde_json::from_value::<ValidationRule>(serde_json::json!({
                "if": "amount > 0",
                "message": "must be positive"
            }))
            .unwrap(),
            ValidationRule {
                r#if: "amount > 0".into(),
                message: Some("must be positive".into()),
            }
        );
    }

    #[test]
    fn refuses_empty_or_fieldless_rules() {
        let bad: Result<ValidationRule, _> = serde_json::from_value(serde_json::json!("   "));
        assert!(
            bad.unwrap_err()
                .to_string()
                .contains("non-empty expression")
        );
        let bad: Result<ValidationRule, _> = serde_json::from_value(serde_json::json!({}));
        assert!(bad.unwrap_err().to_string().contains("needs an 'if'"));
        let bad: Result<ValidationRule, _> =
            serde_json::from_value(serde_json::json!({ "key": "v" }));
        assert!(bad.unwrap_err().to_string().contains("unknown validation"));
    }

    #[test]
    fn round_trips_through_serde() {
        for rule in [
            ValidationRule {
                r#if: "amount > 0".into(),
                message: None,
            },
            ValidationRule {
                r#if: "amount > 0".into(),
                message: Some("must be positive".into()),
            },
        ] {
            let text = serde_json::to_string(&rule).unwrap();
            assert_eq!(serde_json::from_str::<ValidationRule>(&text).unwrap(), rule);
        }
    }

    #[test]
    fn a_rule_without_a_message_fails_with_its_expression() {
        assert_eq!(
            ValidationRule {
                r#if: "amount > 0".into(),
                message: None,
            }
            .failure_message(),
            "validation failed: amount > 0"
        );
        assert_eq!(
            ValidationRule {
                r#if: "amount > 0".into(),
                message: Some("must be positive".into()),
            }
            .failure_message(),
            "must be positive"
        );
    }
}
