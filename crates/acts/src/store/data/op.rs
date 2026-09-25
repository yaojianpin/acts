use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use strum::AsRefStr;

use crate::Result;
use crate::store::{DbCollectionIden, StoreIden};

/// The operation an outbox record represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, AsRefStr)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum OpType {
    /// execute a task whose in-memory scheduler queue was full
    Exec,
    /// propagate the task's `next` (schedule children / move to next node)
    Next,
    /// a client action (event + options) that must be replayed if the engine
    /// crashed before the task state write became durable
    Action,
    /// propagate an unhandled error to the target (normally the parent)
    Error,
    /// propagate an abort to the target (normally the parent)
    Abort,
}

/// Durable outbox record lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, AsRefStr)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum OpStatus {
    /// the operation is enqueued but not yet durably completed
    Pending,
    /// the operation has been handed to the in-memory scheduler; boot recovery
    /// still replays it because the in-memory handoff is not itself durable
    Dispatched,
    /// a scheduler overflow descriptor awaiting replay from disk
    Overflow,
    /// the operation completed and its effects are durable
    Done,
}

/// The six externally meaningful lifecycle phases of one operation.
///
/// `Overflow` is deliberately not here: it is a storage scheduler placement
/// (the in-memory queue was full), not a point in the operation contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, AsRefStr)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum OpPhase {
    /// accepted in memory, not yet acknowledged as durable
    Created,
    /// the intent and all causal inputs are durable and recoverable
    DurablePending,
    /// handed to a worker
    Dispatched,
    /// its business/effect work has started
    EffectInFlight,
    /// the effect and its recovery marker are durable, but the outbox is open
    EffectDurable,
    /// the effect is durable and the outbox record is closed
    Completed,
}

impl OpPhase {
    fn rank(self) -> u8 {
        match self {
            Self::Created => 0,
            Self::DurablePending => 1,
            Self::Dispatched => 2,
            Self::EffectInFlight => 3,
            Self::EffectDurable => 4,
            Self::Completed => 5,
        }
    }

    /// Forward-only lifecycle transition. Recovery may keep a phase, but can
    /// never move an applied effect backwards merely because an outbox close
    /// was lost.
    pub fn can_advance_to(self, next: Self) -> bool {
        next.rank() > self.rank()
    }

    pub fn is_applied(self) -> bool {
        matches!(self, Self::EffectDurable | Self::Completed)
    }

    fn from_status(status: &str) -> Self {
        match status {
            "dispatched" | "overflow" => Self::Dispatched,
            "done" => Self::Completed,
            _ => Self::DurablePending,
        }
    }

    pub fn from_task_value(value: &str) -> Self {
        serde_json::from_value(JsonValue::String(value.to_string())).unwrap_or(Self::DurablePending)
    }
}
/// Durable outbox record for a task operation (`next` propagation or a client
/// action).
///
/// The record is queued on the store writer **before** the in-memory dispatch
/// (FIFO-ordered after the task state write), and is marked `Done` only after
/// the operation's effects (including the `NEXT_COMPLETE` marker) are durably
/// persisted. Crash recovery replays every record that is not `Done`, which
/// makes task operations idempotent across engine restarts: an operation is
/// never lost, and a completed operation is never re-executed.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Op {
    pub id: String,
    pub pid: String,
    /// The task whose outcome is propagated. This is normally the same as
    /// `tid`; the explicit name keeps the direction of a propagation record
    /// unambiguous now that a record can name its target.
    pub source_tid: String,
    pub tid: String,
    /// The target of a propagation hop. `None` for `Exec` and client `Action`
    /// records, which are scoped to `tid` rather than an edge.
    #[serde(default)]
    pub target_tid: Option<String>,
    /// The source task generation being propagated. Recovery uses this to
    /// reject an effect written for an older re-entry of the same task id.
    #[serde(default)]
    pub source_version: i64,
    /// The durable operation that caused this one, when this record was
    /// created as the next hop of a propagation chain.
    #[serde(default)]
    pub causal_op_id: Option<String>,
    /// The operation lifecycle phase. Kept beside the scheduler-facing
    /// `status` so overflow can be represented without polluting the public
    /// six-phase contract.
    #[serde(default)]
    pub phase: String,
    pub r#type: String,
    pub status: String,
    /// event of the recorded client action (`OpType::Action` records only)
    #[serde(default)]
    pub event: Option<String>,
    /// options JSON of the recorded client action (`OpType::Action` records only)
    #[serde(default)]
    pub options: Option<String>,
    pub create_time: i64,
    pub update_time: i64,
    pub v: i32,
}

impl DbCollectionIden for Op {
    fn iden() -> String {
        StoreIden::Ops.as_ref().to_string()
    }
    fn indexed_fields() -> &'static [&'static str] {
        &["pid", "tid", "status"]
    }
    fn version() -> i32 {
        2
    }

    fn upcast(value: JsonValue) -> Result<Self> {
        let v = value.get("v").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        match v {
            2 if v == Self::version() => Self::upcast_current(value),
            1 => Self::upcast_current({
                let mut value = value;
                if let JsonValue::Object(ref mut map) = value {
                    let phase = OpPhase::from_status(
                        map.get("status")
                            .and_then(JsonValue::as_str)
                            .unwrap_or("pending"),
                    );
                    map.insert(
                        "v".to_string(),
                        JsonValue::Number(serde_json::Number::from(Self::version())),
                    );
                    map.insert(
                        "phase".to_string(),
                        JsonValue::String(phase.as_ref().to_string()),
                    );
                }
                value
            }),
            0 => Self::upcast_current({
                let mut value = value;
                if let JsonValue::Object(ref mut map) = value {
                    map.insert(
                        "v".to_string(),
                        JsonValue::Number(serde_json::Number::from(Self::version())),
                    );
                    map.insert(
                        "source_tid".to_string(),
                        map.get("tid").cloned().unwrap_or(JsonValue::Null),
                    );
                    map.entry("target_tid".to_string())
                        .or_insert(JsonValue::Null);
                    map.entry("source_version".to_string())
                        .or_insert(JsonValue::Number(0.into()));
                    map.entry("causal_op_id".to_string())
                        .or_insert(JsonValue::Null);
                    let status = map
                        .get("status")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("pending");
                    map.insert(
                        "phase".to_string(),
                        JsonValue::String(OpPhase::from_status(status).as_ref().to_string()),
                    );
                }
                value
            }),
            _ => Err(crate::ActError::Store(format!(
                "unsupported op version: {}",
                v
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn op_version_returns_current() {
        assert_eq!(Op::version(), 2);
    }

    #[test]
    fn op_phase_is_forward_only() {
        let phases = [
            OpPhase::Created,
            OpPhase::DurablePending,
            OpPhase::Dispatched,
            OpPhase::EffectInFlight,
            OpPhase::EffectDurable,
            OpPhase::Completed,
        ];
        for pair in phases.windows(2) {
            assert!(pair[0].can_advance_to(pair[1]));
            assert!(!pair[1].can_advance_to(pair[0]));
        }
    }

    #[test]
    fn op_upcast_v0_defaults_propagation_fields() {
        let mut map = serde_json::Map::new();
        map.insert("id".to_string(), JsonValue::String("o1".to_string()));
        map.insert("pid".to_string(), JsonValue::String("p1".to_string()));
        map.insert("tid".to_string(), JsonValue::String("t1".to_string()));
        map.insert(
            "type".to_string(),
            JsonValue::String(OpType::Next.as_ref().to_string()),
        );
        map.insert(
            "status".to_string(),
            JsonValue::String(OpStatus::Pending.as_ref().to_string()),
        );
        map.insert(
            "create_time".to_string(),
            JsonValue::Number(serde_json::Number::from(0)),
        );
        map.insert(
            "update_time".to_string(),
            JsonValue::Number(serde_json::Number::from(0)),
        );
        // v field intentionally missing: this is the v0 row shape.

        let op = Op::upcast(JsonValue::Object(map)).unwrap();
        assert_eq!(op.pid, "p1");
        assert_eq!(op.status, OpStatus::Pending.as_ref());
        assert_eq!(op.v, 2);
        assert_eq!(op.source_tid, "t1");
        assert_eq!(op.target_tid, None);
        assert_eq!(op.source_version, 0);
        assert_eq!(op.phase, OpPhase::DurablePending.as_ref());
    }

    #[test]
    fn op_upcast_unknown_version_fails() {
        let mut map = serde_json::Map::new();
        map.insert("id".to_string(), JsonValue::String("o1".to_string()));
        map.insert("pid".to_string(), JsonValue::String("p1".to_string()));
        map.insert("tid".to_string(), JsonValue::String("t1".to_string()));
        map.insert(
            "type".to_string(),
            JsonValue::String(OpType::Next.as_ref().to_string()),
        );
        map.insert(
            "status".to_string(),
            JsonValue::String(OpStatus::Pending.as_ref().to_string()),
        );
        map.insert(
            "create_time".to_string(),
            JsonValue::Number(serde_json::Number::from(0)),
        );
        map.insert(
            "update_time".to_string(),
            JsonValue::Number(serde_json::Number::from(0)),
        );
        map.insert(
            "v".to_string(),
            JsonValue::Number(serde_json::Number::from(99)),
        );

        let result = Op::upcast(JsonValue::Object(map));
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unsupported op version: 99")
        );
    }
}
