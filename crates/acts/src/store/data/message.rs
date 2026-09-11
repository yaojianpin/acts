use core::fmt;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use serde_repr::{Deserialize_repr, Serialize_repr};

use crate::{
    MessageState, Result,
    store::{DbCollectionIden, StoreIden},
};

/// Lifecycle of ONE delivery of a message to one channel — distinct from the
/// message's own state ([`MessageState`](crate::MessageState)), which comes
/// from the task: a message is done when it has no deliveries (own state is
/// terminal) or when every one of its deliveries reached its final state.
///
/// ```text
/// Created ──► Delivered ──► Acked ──► Completed   (final: engine closes)
///    │            │            │            ▲
///    └────────────┴────────────┴────────────┘   (task/message close marks
///    (any) ───────► Error      (retries exhausted — manual resend/clear)
/// ```
///
/// `Acked` is only an intermediate state (the client confirmed receipt); the
/// final state is `Completed` — the engine closed the delivery because the
/// task/message finished. A process is deleted only after it is finished and
/// every delivery is `Completed` (or it has no delivery rows): `Error` rows
/// keep it alive for manual handling.
#[derive(Default, Debug, Copy, PartialEq, Clone, Serialize_repr, Deserialize_repr)]
#[repr(i8)]
pub enum DeliveryStatus {
    #[default]
    Created = 0,
    Acked = 1,
    Completed = 2,
    Error = 3,
    /// Delivered = 4: the delivery row was handed to the channel's handler
    /// and the handler ran to completion — the delivery succeeded. Distinct
    /// from `Created` (row stored but not yet successfully handed over, e.g.
    /// no handler is registered or the engine crashed mid-dispatch): a
    /// `Delivered` delivery only needs an ack (or the task close) to finish,
    /// a `Created` one still needs to be (re-)dispatched.
    Delivered = 4,
}

/// Canonical emitted message — one row per message id. It records the message
/// event once (payload + workflow context); delivery state lives in the
/// separated [`Delivery`](super::Delivery) rows, one per (message × channel).
#[derive(Default, Deserialize, Serialize, Debug, Clone)]
pub struct Message {
    /// the workflow message id — unique key of this emitted event message
    pub id: String,
    pub tid: String,
    pub name: String,
    pub state: MessageState,
    pub r#type: String,
    pub pid: String,
    pub nid: String,
    pub mid: String,
    pub uses: Option<String>,
    pub inputs: String,
    pub outputs: String,
    pub start_time: i64,
    pub end_time: i64,

    pub create_time: i64,
    pub timestamp: i64,
    pub v: i32,
}

impl DbCollectionIden for Message {
    fn iden() -> StoreIden {
        StoreIden::Messages
    }
    fn indexed_fields() -> &'static [&'static str] {
        &["pid", "tid", "nid", "timestamp"]
    }
    fn ordered_index_fields() -> &'static [&'static str] {
        &["timestamp"]
    }
    fn version() -> i32 {
        2
    }

    fn upcast(value: JsonValue) -> Result<Self> {
        let v = value.get("v").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        match v {
            2 => Self::upcast_current(value),
            1 => Self::migrate_v1(value),
            0 => {
                // v0 → v1: move 'tag' field into inputs.options
                let mut value = value;
                if let JsonValue::Object(map) = &mut value {
                    let tag = map.remove("tag");
                    if let Some(tag) = tag
                        && let Some(JsonValue::String(inputs_str)) = map.get("inputs")
                    {
                        let mut inputs_map: serde_json::Map<String, JsonValue> =
                            serde_json::from_str(inputs_str).unwrap_or_default();
                        let options = inputs_map
                            .entry("options".to_string())
                            .or_insert_with(|| JsonValue::Object(serde_json::Map::new()));
                        if let JsonValue::Object(opts) = options {
                            opts.insert("tag".to_string(), tag);
                        }
                        map.insert(
                            "inputs".to_string(),
                            JsonValue::String(
                                serde_json::to_string(&inputs_map).unwrap_or_default(),
                            ),
                        );
                    }
                }
                Self::migrate_v1(value)
            }
            _ => Err(crate::ActError::Store(format!(
                "unsupported message version: {}",
                v
            ))),
        }
    }
}

impl Message {
    /// v1 → v2: v1 rows were merged delivery records keyed by the message id
    /// (id == msg id, single delivery, embedded channel/status fields).
    /// Dropping the delivery-only fields yields the canonical message row —
    /// the row key stays the message id, delivery state moves to the
    /// separated `deliveries` collection. Extra JSON fields are ignored by
    /// serde, only the version needs bumping.
    fn migrate_v1(mut value: JsonValue) -> Result<Self> {
        if let JsonValue::Object(map) = &mut value {
            map.insert(
                "v".to_string(),
                JsonValue::Number(serde_json::Number::from(2)),
            );
        }
        Self::upcast_current(value)
    }
}

impl fmt::Display for DeliveryStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DeliveryStatus::Created => "created",
            DeliveryStatus::Acked => "acked",
            DeliveryStatus::Completed => "completed",
            DeliveryStatus::Error => "error",
            DeliveryStatus::Delivered => "delivered",
        })
    }
}

impl From<i8> for DeliveryStatus {
    fn from(value: i8) -> Self {
        match value {
            1 => DeliveryStatus::Acked,
            2 => DeliveryStatus::Completed,
            3 => DeliveryStatus::Error,
            4 => DeliveryStatus::Delivered,
            _ => DeliveryStatus::Created,
        }
    }
}

impl From<DeliveryStatus> for i8 {
    fn from(val: DeliveryStatus) -> i8 {
        match val {
            DeliveryStatus::Created => 0,
            DeliveryStatus::Acked => 1,
            DeliveryStatus::Completed => 2,
            DeliveryStatus::Error => 3,
            DeliveryStatus::Delivered => 4,
        }
    }
}

impl From<DeliveryStatus> for i64 {
    fn from(val: DeliveryStatus) -> Self {
        match val {
            DeliveryStatus::Created => 0,
            DeliveryStatus::Acked => 1,
            DeliveryStatus::Completed => 2,
            DeliveryStatus::Error => 3,
            DeliveryStatus::Delivered => 4,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value as JsonValue;

    #[test]
    fn store_data_message_status_to_i8() {
        let created: i8 = DeliveryStatus::Created.into();
        assert_eq!(created, 0);

        let created: i8 = DeliveryStatus::Acked.into();
        assert_eq!(created, 1);

        let created: i8 = DeliveryStatus::Completed.into();
        assert_eq!(created, 2);

        let created: i8 = DeliveryStatus::Error.into();
        assert_eq!(created, 3);

        let created: i8 = DeliveryStatus::Delivered.into();
        assert_eq!(created, 4);
    }

    #[test]
    fn store_data_i8_to_message_status() {
        let created: DeliveryStatus = 0.into();
        assert_eq!(created, DeliveryStatus::Created);

        let created: DeliveryStatus = 1.into();
        assert_eq!(created, DeliveryStatus::Acked);

        let created: DeliveryStatus = 2.into();
        assert_eq!(created, DeliveryStatus::Completed);

        let created: DeliveryStatus = 3.into();
        assert_eq!(created, DeliveryStatus::Error);

        let created: DeliveryStatus = 4.into();
        assert_eq!(created, DeliveryStatus::Delivered);

        let created: DeliveryStatus = 100.into();
        assert_eq!(created, DeliveryStatus::Created);
    }

    #[test]
    fn store_data_message_status_to_string() {
        assert_eq!(DeliveryStatus::Created.to_string(), "created");
        assert_eq!(DeliveryStatus::Acked.to_string(), "acked");
        assert_eq!(DeliveryStatus::Completed.to_string(), "completed");
        assert_eq!(DeliveryStatus::Error.to_string(), "error");
        assert_eq!(DeliveryStatus::Delivered.to_string(), "delivered");
    }

    fn message_json(v: i32, extra: bool) -> JsonValue {
        let mut map = serde_json::Map::new();
        map.insert("id".to_string(), JsonValue::String("m1".to_string()));
        map.insert("tid".to_string(), JsonValue::String("t1".to_string()));
        map.insert("name".to_string(), JsonValue::String("test".to_string()));
        map.insert(
            "state".to_string(),
            JsonValue::String("completed".to_string()),
        );
        map.insert("type".to_string(), JsonValue::String("step".to_string()));
        map.insert("pid".to_string(), JsonValue::String("p1".to_string()));
        map.insert("nid".to_string(), JsonValue::String("n1".to_string()));
        map.insert("mid".to_string(), JsonValue::String("mid1".to_string()));
        map.insert("uses".to_string(), JsonValue::String("pack".to_string()));
        map.insert("inputs".to_string(), JsonValue::String("{}".to_string()));
        map.insert("outputs".to_string(), JsonValue::String("{}".to_string()));
        map.insert(
            "start_time".to_string(),
            JsonValue::Number(serde_json::Number::from(0)),
        );
        map.insert(
            "end_time".to_string(),
            JsonValue::Number(serde_json::Number::from(0)),
        );
        map.insert(
            "create_time".to_string(),
            JsonValue::Number(serde_json::Number::from(0)),
        );
        map.insert(
            "timestamp".to_string(),
            JsonValue::Number(serde_json::Number::from(1000)),
        );
        if extra {
            // legacy merged-delivery fields — must be dropped on upcast
            map.insert("chan_id".to_string(), JsonValue::String("ch1".to_string()));
            map.insert(
                "chan_pattern".to_string(),
                JsonValue::String("*:*:*:*".to_string()),
            );
            map.insert("msg_id".to_string(), JsonValue::String("m1".to_string()));
            map.insert(
                "status".to_string(),
                JsonValue::Number(serde_json::Number::from(0)),
            );
            map.insert(
                "retry_times".to_string(),
                JsonValue::Number(serde_json::Number::from(3)),
            );
            map.insert(
                "update_time".to_string(),
                JsonValue::Number(serde_json::Number::from(2000)),
            );
        }
        map.insert(
            "v".to_string(),
            JsonValue::Number(serde_json::Number::from(v)),
        );
        JsonValue::Object(map)
    }

    #[test]
    fn upcast_v0_with_tag_strips_tag() {
        let mut map = match message_json(0, false) {
            JsonValue::Object(map) => map,
            _ => unreachable!(),
        };
        map.insert("tag".to_string(), JsonValue::String("old-tag".to_string()));

        let msg = Message::upcast(JsonValue::Object(map)).unwrap();
        assert_eq!(msg.id, "m1");
        assert_eq!(msg.v, 2);
        // verify tag moved into inputs.options
        let inputs: JsonValue = serde_json::from_str(&msg.inputs).unwrap();
        assert_eq!(inputs["options"]["tag"].as_str().unwrap(), "old-tag");
    }

    #[test]
    fn upcast_v1_merged_row_becomes_canonical() {
        let msg = Message::upcast(message_json(1, true)).unwrap();
        assert_eq!(msg.id, "m1");
        assert_eq!(msg.v, 2);
    }

    #[test]
    fn upcast_v2_canonical_passes_through() {
        let msg = Message::upcast(message_json(2, false)).unwrap();
        assert_eq!(msg.id, "m1");
        assert_eq!(msg.v, 2);
    }
}
