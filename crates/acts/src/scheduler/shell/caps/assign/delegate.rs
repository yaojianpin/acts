//! The delegation state of one human act and its rules.
//!
//! The durable `#delegation` fact ([`Delegation`], at most one active per
//! act) with the pure helpers around it — validity at a clock, the transport
//! shapes of `expires_at` and `expect_rev`, and the payload projection — as
//! methods on [`Assign`](super::Assign). The shell keeps the durable reads
//! and the caller-facing checks (see `scheduler::shell::assign`).

use crate::{ActError, Result};

/// The private execution variable carrying one human act's active delegation:
/// `{"id","delegator","delegatee","granted_by","expires_at","reason"}`, or
/// `null` once revoked. It is the authorization source for a delegatee; the
/// message fields are only its projection.
pub(crate) const DELEGATION_VAR: &str = "#delegation";

/// The per-task delegation of one human decision execution: at most one is
/// active. The variable holds `null` after `act:undelegate` or a transfer, so
/// an old delegatee is refused even though the scope keeps a record.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct Delegation {
    #[serde(default)]
    pub(crate) id: String,
    pub(crate) delegator: String,
    pub(crate) delegatee: String,
    #[serde(default)]
    pub(crate) granted_by: Option<String>,
    #[serde(default)]
    pub(crate) expires_at: Option<i64>,
    #[serde(default)]
    pub(crate) reason: Option<String>,
}

impl Delegation {
    /// The delegation one `act:delegate` writes: the delegator is the current
    /// owner (the engine's fact, never a client field) and `granted_by` is the
    /// authenticated caller.
    pub(crate) fn new(
        id: String,
        delegator: &str,
        delegatee: &str,
        granted_by: Option<&str>,
        expires_at: Option<i64>,
        reason: Option<&str>,
    ) -> Self {
        Self {
            id,
            delegator: delegator.to_string(),
            delegatee: delegatee.to_string(),
            granted_by: granted_by.map(str::to_string),
            expires_at,
            reason: reason.map(str::to_string),
        }
    }

    /// Whether the delegation still admits the delegatee at `now`. An absent
    /// `expires_at` never expires; expiry is checked when a decision is made,
    /// never by a background sweep.
    pub(crate) fn is_valid(&self, now: i64) -> bool {
        self.expires_at.is_none_or(|expires_at| now < expires_at)
    }

    /// The aggregate's JSON-text representation (a scope variable value).
    pub(crate) fn value(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "null".to_string())
    }
}

impl super::Assign {
    /// Parse a delegation's optional `expires_at` (a JSON number or numeric
    /// string, the shapes the transports carry). A malformed value is refused
    /// before anything durable exists; the decision path later compares it
    /// with its own clock.
    pub(crate) fn parse_expires_at(raw: &str) -> Result<i64> {
        let raw = raw.trim();
        raw.parse::<i64>()
            .or_else(|_| {
                serde_json::from_str::<serde_json::Value>(raw)
                    .ok()
                    .and_then(|value| match value {
                        serde_json::Value::Number(number) => number.as_i64(),
                        serde_json::Value::String(text) => text.parse().ok(),
                        _ => None,
                    })
                    .ok_or(())
            })
            .map_err(|_| ActError::Action(format!("invalid expires_at '{raw}' in act:delegate")))
    }

    /// Parse a transfer's `expect_rev` (a JSON number or numeric string, the
    /// shapes the transports carry). A malformed value is refused like a
    /// stale revision: it can never authorize a transfer.
    pub(crate) fn parse_expect_rev(raw: &str) -> Result<u64> {
        let raw = raw.trim();
        raw.parse::<u64>().or_else(|_| {
            serde_json::from_str::<serde_json::Value>(raw)
                .ok()
                .and_then(|value| match value {
                    serde_json::Value::Number(number) => number.as_u64(),
                    serde_json::Value::String(text) => text.parse().ok(),
                    _ => None,
                })
                .ok_or_else(|| {
                    ActError::Action(format!("EASSIGNMENT_CHANGED: invalid expect_rev '{raw}'"))
                })
        })
    }

    /// Project an active delegation's delegator onto a payload that already
    /// carries the assignment fields. The owner and assignee stay what
    /// [`Assign::inject_payload_assignment`] wrote; the delegatee is not a
    /// separate field — the act's *assignee* is whoever executes it now.
    pub(crate) fn inject_payload_delegation(payload: &mut serde_json::Value, delegator: &str) {
        payload["delegator"] = serde_json::Value::String(delegator.to_string());
    }
}
