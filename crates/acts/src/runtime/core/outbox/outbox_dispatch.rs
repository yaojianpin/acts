use crate::runtime::core::outbox::records::OutboxRecord;

/// Result of one external delivery attempt. The transport reports facts only;
/// retry timing and quarantine policy remain application decisions.
///
/// REGISTERED EXCEPTION (KERNEL_CUTOVER_NOTES.md §10, P4a): the attached
/// transport in production is the message forwarder, which only ever reports
/// `Delivered`, so `RetryAt`/`OutcomeUnknown`/`Quarantine` are never
/// constructed in-tree — yet `KernelHost::dispatch_outbox` handles all four
/// exhaustively and turns each into a durable `OutboxEvent`
/// (`Reschedule`/`MarkDeliveryUnknown`/`Quarantine`). The enum is the transport
/// contract those handlers exist for; deleting the three would delete the
/// engine's retry/quarantine handling with them.
#[allow(dead_code)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DeliveryOutcome {
    Delivered { receipt: String },
    RetryAt { available_at_ms: i64, error: String },
    OutcomeUnknown { reason: String },
    Quarantine { reason: String },
}

pub(crate) trait OutboxTransport: Send + Sync {
    fn deliver(&mut self, record: &OutboxRecord) -> DeliveryOutcome;
}
