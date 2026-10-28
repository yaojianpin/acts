//! The outbox-delivery port: handing a durable record to an external system.
//!
//! Delivery reports facts only; retry timing and quarantine policy remain the
//! kernel's decisions. The shell's `MessageForwarder` is the serving
//! implementation, the shadow transport is the kernel's default.

use crate::scheduler::OutboxRecord;

/// Result of one external delivery attempt. The transport reports facts only;
/// retry timing and quarantine policy remain application decisions.
///
/// REGISTERED EXCEPTION: the attached transport in production is the message
/// forwarder, which only ever reports `Delivered`, so
/// `RetryAt`/`OutcomeUnknown`/`Quarantine` are never constructed in-tree — yet
/// the host's dispatch handles all four exhaustively and turns each into a
/// durable `OutboxEvent`. The enum is the transport contract those handlers
/// exist for; deleting the three would delete the engine's retry/quarantine
/// handling with them.
#[allow(dead_code)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DeliveryOutcome {
    Delivered { receipt: String },
    RetryAt { available_at_ms: i64, error: String },
    OutcomeUnknown { reason: String },
    Quarantine { reason: String },
}

/// Delivers one durable outbox record.
///
/// The host calls this synchronously, never across an await; an implementation
/// that needs mutable state keeps it behind its own interior mutability, so a
/// port handle can be shared.
pub(crate) trait OutboxTransport: Send + Sync {
    fn deliver(&self, record: &OutboxRecord) -> DeliveryOutcome;
}
