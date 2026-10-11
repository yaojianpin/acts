use super::outbox::{DeliveryClaim, OutboxEvent, OutboxId, OutboxRecord};
use super::outbox_repository::{OutboxRepository, OutboxRepositoryError};

/// Result of one external delivery attempt. The transport reports facts only;
/// retry timing and quarantine policy remain application decisions.
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DispatchRequest {
    pub(crate) id: OutboxId,
    pub(crate) expected_revision: u64,
    pub(crate) owner: String,
    pub(crate) lease_until_ms: i64,
    pub(crate) now_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DispatchResult {
    Delivered(OutboxRecord),
    Rescheduled(OutboxRecord),
    OutcomeUnknown(OutboxRecord),
    Quarantined(OutboxRecord),
}

/// Claims one record using repository CAS, performs the external effect, then
/// persists the reported outcome using the claimed revision. An unclassified
/// transport error is conservatively persisted as `DeliveryUnknown`: after the
/// transport was invoked, only the transport can explicitly prove `RetryAt`
/// means no externally visible effect occurred.
pub(crate) fn dispatch_one<R, T>(
    repository: &mut R,
    transport: &mut T,
    request: DispatchRequest,
) -> Result<DispatchResult, DispatchError>
where
    R: OutboxRepository,
    T: OutboxTransport,
{
    if request.owner.trim().is_empty() {
        return Err(DispatchError::InvalidOwner);
    }
    if request.lease_until_ms <= request.now_ms {
        return Err(DispatchError::InvalidLease);
    }
    let current = repository
        .load_outbox(&request.id)?
        .ok_or_else(|| DispatchError::NotFound(request.id.clone()))?;
    if current.revision != request.expected_revision {
        return Err(DispatchError::StaleRequest {
            expected: request.expected_revision,
            actual: current.revision,
        });
    }
    let fence = current
        .last_fence
        .checked_add(1)
        .ok_or(DispatchError::FenceOverflow)?;
    let claim = DeliveryClaim {
        owner: request.owner,
        fence,
        lease_until_ms: request.lease_until_ms,
        heartbeat_revision: 0,
    };
    let claimed = repository.evolve_outbox(
        &request.id,
        current.revision,
        OutboxEvent::Claim {
            claim: claim.clone(),
            now_ms: request.now_ms,
        },
    )?;

    let outcome = transport.deliver(&claimed);
    let event = match outcome {
        DeliveryOutcome::Delivered { receipt } => OutboxEvent::ConfirmDelivered {
            claim,
            receipt: bounded_diagnostic("delivery receipt", &receipt),
            now_ms: request.now_ms,
        },
        DeliveryOutcome::RetryAt {
            available_at_ms,
            error,
        } => OutboxEvent::Reschedule {
            claim,
            available_at_ms,
            error: bounded_diagnostic("transport retry", &error),
            now_ms: request.now_ms,
        },
        DeliveryOutcome::OutcomeUnknown { reason } => OutboxEvent::MarkDeliveryUnknown {
            claim,
            reason: bounded_diagnostic("delivery outcome unknown", &reason),
            now_ms: request.now_ms,
        },
        DeliveryOutcome::Quarantine { reason } => OutboxEvent::Quarantine {
            reason: bounded_diagnostic("delivery quarantined", &reason),
        },
    };
    let evolved = repository.evolve_outbox(&request.id, claimed.revision, event)?;
    Ok(match evolved.state {
        super::outbox::OutboxState::Delivered { .. } => DispatchResult::Delivered(evolved),
        super::outbox::OutboxState::Pending { .. } => DispatchResult::Rescheduled(evolved),
        super::outbox::OutboxState::DeliveryUnknown { .. } => {
            DispatchResult::OutcomeUnknown(evolved)
        }
        super::outbox::OutboxState::Quarantined { .. } => DispatchResult::Quarantined(evolved),
        super::outbox::OutboxState::Claimed(_) => {
            unreachable!("an outcome cannot preserve Claimed")
        }
    })
}

fn bounded_diagnostic(context: &str, detail: &str) -> String {
    let prefix = format!("{context}: ");
    let max = super::outbox::MAX_OUTBOX_DIAGNOSTIC_BYTES.saturating_sub(prefix.len());
    let mut end = detail.len().min(max);
    while !detail.is_char_boundary(end) {
        end -= 1;
    }
    let detail = detail[..end].trim();
    if detail.is_empty() {
        format!("{context}: unspecified")
    } else {
        format!("{prefix}{detail}")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DispatchError {
    InvalidOwner,
    InvalidLease,
    FenceOverflow,
    NotFound(OutboxId),
    StaleRequest { expected: u64, actual: u64 },
    Repository(OutboxRepositoryError),
}
impl From<OutboxRepositoryError> for DispatchError {
    fn from(value: OutboxRepositoryError) -> Self {
        Self::Repository(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::outbox::{OutboxId, OutboxRecord, OutboxState};
    use crate::kernel::outbox_repository::MemoryOutboxRepository;

    #[derive(Default)]
    struct Transport {
        outcome: Option<DeliveryOutcome>,
    }
    impl OutboxTransport for Transport {
        fn deliver(&mut self, _: &OutboxRecord) -> DeliveryOutcome {
            self.outcome.take().expect("one configured delivery")
        }
    }
    fn repository() -> (MemoryOutboxRepository, OutboxId) {
        let id = OutboxId {
            process_id: "p".into(),
            key: "k".into(),
        };
        let mut repository = MemoryOutboxRepository::default();
        repository
            .insert(OutboxRecord::pending(id.clone(), "topic".into(), "{}".into(), 0).unwrap())
            .unwrap();
        (repository, id)
    }
    fn request(id: OutboxId) -> DispatchRequest {
        DispatchRequest {
            id,
            expected_revision: 0,
            owner: "worker".into(),
            lease_until_ms: 100,
            now_ms: 0,
        }
    }

    #[test]
    fn successful_delivery_is_claimed_then_confirmed() {
        let (mut repository, id) = repository();
        let mut transport = Transport {
            outcome: Some(DeliveryOutcome::Delivered {
                receipt: "r".into(),
            }),
        };
        let result = dispatch_one(&mut repository, &mut transport, request(id.clone())).unwrap();
        assert!(matches!(result, DispatchResult::Delivered(_)));
        assert!(matches!(
            repository.load_outbox(&id).unwrap().unwrap().state,
            OutboxState::Delivered { .. }
        ));
    }

    #[test]
    fn transport_explicitly_classifies_unknown_outcome() {
        let (mut repository, id) = repository();
        let mut transport = Transport {
            outcome: Some(DeliveryOutcome::OutcomeUnknown {
                reason: "connection reset".into(),
            }),
        };
        let result = dispatch_one(&mut repository, &mut transport, request(id.clone())).unwrap();
        assert!(matches!(result, DispatchResult::OutcomeUnknown(_)));
        assert!(matches!(
            repository.load_outbox(&id).unwrap().unwrap().state,
            OutboxState::DeliveryUnknown { .. }
        ));
    }

    #[test]
    fn explicit_unknown_outcome_is_persisted() {
        let (mut repository, id) = repository();
        let mut transport = Transport {
            outcome: Some(DeliveryOutcome::OutcomeUnknown {
                reason: "timeout after write".into(),
            }),
        };
        let result = dispatch_one(&mut repository, &mut transport, request(id.clone())).unwrap();
        assert!(matches!(result, DispatchResult::OutcomeUnknown(_)));
        assert!(matches!(
            repository.load_outbox(&id).unwrap().unwrap().state,
            OutboxState::DeliveryUnknown { .. }
        ));
    }
    #[test]
    fn retry_outcome_uses_application_selected_time() {
        let (mut repository, id) = repository();
        let mut transport = Transport {
            outcome: Some(DeliveryOutcome::RetryAt {
                available_at_ms: 500,
                error: "temporary".into(),
            }),
        };
        let result = dispatch_one(&mut repository, &mut transport, request(id.clone())).unwrap();
        assert!(matches!(result, DispatchResult::Rescheduled(_)));
        let saved = repository.load_outbox(&id).unwrap().unwrap();
        assert!(!saved.state.is_dispatchable(499));
        assert!(saved.state.is_dispatchable(500));
    }

    #[test]
    fn transport_can_explicitly_quarantine_poison_message() {
        let (mut repository, id) = repository();
        let mut transport = Transport {
            outcome: Some(DeliveryOutcome::Quarantine {
                reason: "invalid destination".into(),
            }),
        };
        let result = dispatch_one(&mut repository, &mut transport, request(id.clone())).unwrap();
        assert!(matches!(result, DispatchResult::Quarantined(_)));
        assert!(
            repository
                .load_outbox(&id)
                .unwrap()
                .unwrap()
                .state
                .is_terminal()
        );
    }

    #[test]
    fn oversized_unicode_transport_error_is_safely_bounded() {
        struct LargeErrorTransport;
        impl OutboxTransport for LargeErrorTransport {
            fn deliver(&mut self, _: &OutboxRecord) -> DeliveryOutcome {
                DeliveryOutcome::OutcomeUnknown {
                    reason: "故障".repeat(4096),
                }
            }
        }
        let (mut repository, id) = repository();
        let result = dispatch_one(
            &mut repository,
            &mut LargeErrorTransport,
            request(id.clone()),
        )
        .unwrap();
        assert!(matches!(result, DispatchResult::OutcomeUnknown(_)));
        let saved = repository.load_outbox(&id).unwrap().unwrap();
        saved.validate().unwrap();
    }

    #[test]
    fn stale_request_never_invokes_transport() {
        struct PanicTransport;
        impl OutboxTransport for PanicTransport {
            fn deliver(&mut self, _: &OutboxRecord) -> DeliveryOutcome {
                panic!("stale request must not invoke transport")
            }
        }
        let (mut repository, id) = repository();
        let mut stale = request(id);
        stale.expected_revision = 1;
        assert!(matches!(
            dispatch_one(&mut repository, &mut PanicTransport, stale),
            Err(DispatchError::StaleRequest { actual: 0, .. })
        ));
    }
    #[test]
    fn blank_transport_diagnostics_are_normalized_before_persistence() {
        let (mut repository, id) = repository();
        let mut transport = Transport {
            outcome: Some(DeliveryOutcome::OutcomeUnknown {
                reason: "   ".into(),
            }),
        };
        dispatch_one(&mut repository, &mut transport, request(id.clone())).unwrap();
        assert_eq!(
            repository.load_outbox(&id).unwrap().unwrap().state,
            OutboxState::DeliveryUnknown {
                reason: "delivery outcome unknown: unspecified".into()
            }
        );
    }

    #[test]
    fn oversized_unicode_receipt_is_bounded_without_splitting_utf8() {
        let (mut repository, id) = repository();
        let mut transport = Transport {
            outcome: Some(DeliveryOutcome::Delivered {
                receipt: "完成".repeat(4096),
            }),
        };
        dispatch_one(&mut repository, &mut transport, request(id.clone())).unwrap();
        let saved = repository.load_outbox(&id).unwrap().unwrap();
        saved.validate().unwrap();
        let OutboxState::Delivered { receipt } = saved.state else {
            panic!("delivery must be terminal");
        };
        assert!(receipt.len() <= crate::kernel::outbox::MAX_OUTBOX_DIAGNOSTIC_BYTES);
        assert!(receipt.is_char_boundary(receipt.len()));
    }

    #[test]
    fn retry_and_unknown_are_distinct_durable_effect_classifications() {
        let (mut retry_repository, retry_id) = repository();
        let mut retry_transport = Transport {
            outcome: Some(DeliveryOutcome::RetryAt {
                available_at_ms: 50,
                error: "not sent".into(),
            }),
        };
        dispatch_one(
            &mut retry_repository,
            &mut retry_transport,
            request(retry_id.clone()),
        )
        .unwrap();
        assert!(matches!(
            retry_repository
                .load_outbox(&retry_id)
                .unwrap()
                .unwrap()
                .state,
            OutboxState::Pending {
                available_at_ms: 50
            }
        ));

        let (mut unknown_repository, unknown_id) = repository();
        let mut unknown_transport = Transport {
            outcome: Some(DeliveryOutcome::OutcomeUnknown {
                reason: "response lost".into(),
            }),
        };
        dispatch_one(
            &mut unknown_repository,
            &mut unknown_transport,
            request(unknown_id.clone()),
        )
        .unwrap();
        assert!(matches!(
            unknown_repository
                .load_outbox(&unknown_id)
                .unwrap()
                .unwrap()
                .state,
            OutboxState::DeliveryUnknown { .. }
        ));
    }
}
