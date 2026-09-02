use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use strum::AsRefStr;

use crate::Result;
use crate::store::{DbCollectionIden, StoreIden};

/// The operation an outbox record represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, AsRefStr)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum OpType {
    /// propagate the task's `next` (schedule children / move to next node)
    Next,
    /// a client action (event + options) that must be replayed if the engine
    /// crashed before the task state write became durable
    Action,
}

/// Durable outbox record lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, AsRefStr)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum OpStatus {
    /// the operation is enqueued but not yet durably completed
    Pending,
    /// the operation completed and its effects are durable
    Done,
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
    pub tid: String,
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
    fn iden() -> StoreIden {
        StoreIden::Ops
    }
    fn indexed_fields() -> &'static [&'static str] {
        &["pid", "tid", "status"]
    }
    fn version() -> i32 {
        0
    }

    fn upcast(value: JsonValue) -> Result<Self> {
        let v = value.get("v").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        if v == Self::version() {
            return Self::upcast_current(value);
        }
        Err(crate::ActError::Store(format!(
            "unsupported op version: {}",
            v
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn op_version_returns_current() {
        assert_eq!(Op::version(), 0);
    }

    #[test]
    fn op_upcast_missing_v_defaults_to_0() {
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
        // v field intentionally missing

        let op = Op::upcast(JsonValue::Object(map)).unwrap();
        assert_eq!(op.pid, "p1");
        assert_eq!(op.status, OpStatus::Pending.as_ref());
        assert_eq!(op.v, 0);
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
