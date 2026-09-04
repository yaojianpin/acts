use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::{
    Result,
    store::{DbCollectionIden, StoreIden},
};

use super::message::MessageStatus;

/// One delivery of a canonical message to one channel/service — the unit
/// Ack/Retry/Clear/Redo and the retry timer operate on. Rows are keyed by
/// their own delivery id and reference the message id; the message payload is
/// stored once in the `messages` collection.
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

    pub status: MessageStatus,
    pub retry_times: i32,
    pub create_time: i64,
    pub update_time: i64,
    /// message event timestamp (microseconds), denormalized so delivery rows
    /// can be ordered by event time without joining the canonical message
    pub timestamp: i64,
    pub v: i32,
}

impl DbCollectionIden for Delivery {
    fn iden() -> StoreIden {
        StoreIden::Deliveries
    }
    fn indexed_fields() -> &'static [&'static str] {
        &["pid", "tid", "status", "msg_id", "chan_id"]
    }
    fn version() -> i32 {
        1
    }

    fn upcast(value: JsonValue) -> Result<Self> {
        Self::upcast_current(value)
    }
}
