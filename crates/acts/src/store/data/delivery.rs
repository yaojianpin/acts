use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::{
    Result,
    store::{DbCollectionIden, StoreIden},
};

use super::message::DeliveryStatus;

/// One delivery of a canonical message to one channel/service — the unit
/// Ack/Retry/Clear/Redo and the retry timer operate on. Rows are keyed by
/// their own delivery id and reference the message id; the message payload is
/// stored once in the `messages` collection.
///
/// Every status transition is a conditional read-modify-write serialized per
/// row (`Store::rewrite_deliveries`): the engine's close, the retry pass and
/// the client's delivery/ack writes decide on the stored row, so none of them
/// can overwrite a state another transition already committed.
#[derive(Default, Deserialize, Serialize, Debug, Clone)]
pub struct Delivery {
    /// delivery id — unique storage key of this (message × channel) delivery
    pub id: String,

    /// the canonical message id this delivery carries
    pub msg_id: String,

    /// process/task ids denormalized for lifecycle operations
    pub pid: String,
    pub tid: String,

    /// the channel the delivery belongs to (e.g. a grpc/SSE client or a
    /// broker adapter such as nats/kafka)
    pub chan_id: String,
    pub chan_pattern: String,

    pub status: DeliveryStatus,
    pub retry_times: i32,
    pub create_time: i64,
    pub update_time: i64,
    /// message event timestamp (microseconds), denormalized so delivery rows
    /// can be ordered by event time without joining the canonical message
    pub timestamp: i64,

    /// The assignment projection the canonical message carries, denormalized
    /// so a delivery read answers routing fields without a join.
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub assignee: Option<String>,
    #[serde(default)]
    pub delegator: Option<String>,
    #[serde(default)]
    pub rev: u64,
    #[serde(default)]
    pub ref_msg_id: Option<String>,

    pub v: i32,
}

impl DbCollectionIden for Delivery {
    fn iden() -> String {
        StoreIden::Deliveries.as_ref().to_string()
    }
    fn indexed_fields() -> &'static [&'static str] {
        &["pid", "tid", "status", "msg_id", "chan_id", "update_time"]
    }
    fn ordered_index_fields() -> &'static [&'static str] {
        &["update_time"]
    }
    fn version() -> i32 {
        2
    }

    fn upcast(value: JsonValue) -> Result<Self> {
        let v = value.get("v").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        match v {
            2 => Self::upcast_current(value),
            // v0/v1 predate the assignment projection; every new field is
            // `#[serde(default)]`, so bumping the version is the migration.
            0 | 1 => {
                let mut value = value;
                if let JsonValue::Object(map) = &mut value {
                    map.insert(
                        "v".to_string(),
                        JsonValue::Number(serde_json::Number::from(2)),
                    );
                }
                Self::upcast_current(value)
            }
            _ => Err(crate::ActError::Store(format!(
                "unsupported delivery version: {v}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivery_index_metadata_and_upcast() {
        assert_eq!(Delivery::iden(), "deliveries");
        assert!(Delivery::indexed_fields().contains(&"status"));
        assert_eq!(Delivery::ordered_index_fields(), &["update_time"]);
        assert_eq!(Delivery::version(), 2);

        let row = Delivery {
            id: "d1".into(),
            msg_id: "m1".into(),
            pid: "p1".into(),
            status: DeliveryStatus::Acked,
            ..Default::default()
        };
        let json = serde_json::to_value(&row).unwrap();
        let restored = Delivery::upcast(json).unwrap();
        assert_eq!(restored.id, "d1");
        assert_eq!(restored.status, DeliveryStatus::Acked);
        assert_eq!(restored.v, 2);
    }

    #[test]
    fn a_v1_delivery_row_upcasts_with_default_assignment_fields() {
        let row = serde_json::json!({
            "id": "d1",
            "msg_id": "m1",
            "pid": "p1",
            "tid": "t1",
            "chan_id": "c1",
            "chan_pattern": "*",
            "status": 0,
            "retry_times": 0,
            "create_time": 1,
            "update_time": 2,
            "timestamp": 3,
            "v": 1
        });
        let restored = Delivery::upcast(row).unwrap();
        assert_eq!(restored.v, 2);
        assert_eq!(restored.owner, None);
        assert_eq!(restored.assignee, None);
        assert_eq!(restored.rev, 0);
        assert_eq!(restored.ref_msg_id, None);
    }

    #[test]
    fn delivery_upcast_rejects_missing_required_field() {
        // `id` is required: upcast reports the deserialization failure.
        assert!(Delivery::upcast(serde_json::json!({"v": 1})).is_err());
    }
}
