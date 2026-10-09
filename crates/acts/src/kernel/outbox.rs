use std::fmt;

pub(crate) const MAX_OUTBOX_PROCESS_ID_BYTES: usize = 1024;
pub(crate) const MAX_OUTBOX_KEY_BYTES: usize = 1024;
pub(crate) const MAX_OUTBOX_OWNER_BYTES: usize = 512;
pub(crate) const MAX_OUTBOX_TOPIC_BYTES: usize = 512;
pub(crate) const MAX_OUTBOX_PAYLOAD_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_OUTBOX_DIAGNOSTIC_BYTES: usize = 4096;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct OutboxId {
    pub(crate) process_id: String,
    pub(crate) key: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DeliveryClaim {
    pub(crate) owner: String,
    pub(crate) fence: u64,
    pub(crate) lease_until_ms: i64,
    pub(crate) heartbeat_revision: u64,
}
impl DeliveryClaim {
    pub(crate) fn is_expired(&self, now_ms: i64) -> bool {
        now_ms >= self.lease_until_ms
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OutboxState {
    Pending { available_at_ms: i64 },
    Claimed(DeliveryClaim),
    DeliveryUnknown { reason: String },
    Delivered { receipt: String },
    Quarantined { reason: String },
}
impl OutboxState {
    pub(crate) fn is_terminal(&self) -> bool {
        matches!(self, Self::Delivered { .. } | Self::Quarantined { .. })
    }
    pub(crate) fn is_dispatchable(&self, now_ms: i64) -> bool {
        matches!(self, Self::Pending { available_at_ms } if now_ms >= *available_at_ms)
    }
}

/// One immutable message and its authoritative delivery lifecycle.
/// Retry policy is deliberately external: this aggregate records only the next
/// application-selected availability instant and never invents backoff or a DLQ.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OutboxRecord {
    pub(crate) id: OutboxId,
    pub(crate) topic: String,
    pub(crate) payload: String,
    pub(crate) state: OutboxState,
    pub(crate) attempt: u32,
    pub(crate) last_fence: u64,
    pub(crate) revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OutboxEvent {
    Claim {
        claim: DeliveryClaim,
        now_ms: i64,
    },
    RenewClaim {
        current: DeliveryClaim,
        renewed: DeliveryClaim,
        now_ms: i64,
    },
    ConfirmDelivered {
        claim: DeliveryClaim,
        receipt: String,
        now_ms: i64,
    },
    Reschedule {
        claim: DeliveryClaim,
        available_at_ms: i64,
        error: String,
        now_ms: i64,
    },
    MarkDeliveryUnknown {
        claim: DeliveryClaim,
        reason: String,
        now_ms: i64,
    },
    ReleaseExpiredClaim {
        now_ms: i64,
    },
    Quarantine {
        reason: String,
    },
}

impl OutboxRecord {
    pub(crate) fn pending(
        id: OutboxId,
        topic: String,
        payload: String,
        available_at_ms: i64,
    ) -> Result<Self, OutboxError> {
        let record = Self {
            id,
            topic,
            payload,
            state: OutboxState::Pending { available_at_ms },
            attempt: 0,
            last_fence: 0,
            revision: 0,
        };
        record.validate()?;
        Ok(record)
    }

    pub(crate) fn validate(&self) -> Result<(), OutboxError> {
        validate_bounded_text(
            &self.id.process_id,
            MAX_OUTBOX_PROCESS_ID_BYTES,
            OutboxError::InvalidProcessId,
        )?;
        validate_bounded_text(&self.id.key, MAX_OUTBOX_KEY_BYTES, OutboxError::InvalidKey)?;
        validate_bounded_text(
            &self.topic,
            MAX_OUTBOX_TOPIC_BYTES,
            OutboxError::InvalidTopic,
        )?;
        if self.payload.len() > MAX_OUTBOX_PAYLOAD_BYTES {
            return Err(OutboxError::PayloadTooLarge);
        }

        if u64::from(self.attempt) != self.last_fence || self.revision < u64::from(self.attempt) {
            return Err(OutboxError::CounterInvariant);
        }

        match &self.state {
            OutboxState::Pending { .. } => {}
            OutboxState::Claimed(claim) => {
                if self.attempt == 0
                    || !valid_persisted_claim(claim)
                    || claim.fence != self.last_fence
                {
                    return Err(OutboxError::InvalidPersistedState);
                }
            }
            OutboxState::DeliveryUnknown { reason } => {
                if self.attempt == 0 || !valid_diagnostic(reason) {
                    return Err(OutboxError::InvalidPersistedState);
                }
            }
            OutboxState::Quarantined { reason } => {
                if !valid_diagnostic(reason) {
                    return Err(OutboxError::InvalidPersistedState);
                }
            }
            OutboxState::Delivered { receipt } => {
                if self.attempt == 0 || !valid_diagnostic(receipt) {
                    return Err(OutboxError::InvalidPersistedState);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn evolve(&self, event: OutboxEvent) -> Result<Self, OutboxError> {
        use OutboxEvent as E;
        use OutboxState as S;
        let mut attempt = self.attempt;
        let mut last_fence = self.last_fence;
        let state = match (&self.state, event) {
            (S::Pending { available_at_ms }, E::Claim { claim, now_ms })
                if now_ms >= *available_at_ms
                    && claim.lease_until_ms > now_ms
                    && valid_initial_claim(&claim, self.last_fence) =>
            {
                attempt = attempt.checked_add(1).ok_or(OutboxError::AttemptOverflow)?;
                last_fence = claim.fence;
                S::Claimed(claim)
            }
            (
                S::Claimed(current),
                E::RenewClaim {
                    current: supplied,
                    renewed,
                    now_ms,
                },
            ) if current == &supplied
                && now_ms < current.lease_until_ms
                && renewed.lease_until_ms > now_ms
                && valid_renewal(current, &renewed) =>
            {
                S::Claimed(renewed)
            }
            (
                S::Claimed(current),
                E::ConfirmDelivered {
                    claim,
                    receipt,
                    now_ms,
                },
            ) if current == &claim
                && now_ms < current.lease_until_ms
                && valid_diagnostic(&receipt) =>
            {
                S::Delivered { receipt }
            }
            (
                S::Claimed(current),
                E::Reschedule {
                    claim,
                    available_at_ms,
                    error,
                    now_ms,
                },
            ) if current == &claim
                && now_ms < current.lease_until_ms
                && valid_diagnostic(&error) =>
            {
                S::Pending { available_at_ms }
            }
            (
                S::Claimed(current),
                E::MarkDeliveryUnknown {
                    claim,
                    reason,
                    now_ms,
                },
            ) if current == &claim
                && now_ms < current.lease_until_ms
                && valid_diagnostic(&reason) =>
            {
                S::DeliveryUnknown { reason }
            }
            (S::Claimed(claim), E::ReleaseExpiredClaim { now_ms }) if claim.is_expired(now_ms) => {
                S::Pending {
                    available_at_ms: now_ms,
                }
            }
            (S::Pending { .. }, E::Quarantine { reason })
            | (S::Claimed(_), E::Quarantine { reason })
            | (S::DeliveryUnknown { .. }, E::Quarantine { reason })
                if valid_diagnostic(&reason) =>
            {
                S::Quarantined { reason }
            }
            _ => return Err(OutboxError::InvalidTransition),
        };
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(OutboxError::RevisionOverflow)?;
        let record = Self {
            id: self.id.clone(),
            topic: self.topic.clone(),
            payload: self.payload.clone(),
            state,
            attempt,
            last_fence,
            revision,
        };
        record.validate()?;
        Ok(record)
    }
}

fn validate_bounded_text(value: &str, max: usize, error: OutboxError) -> Result<(), OutboxError> {
    if value.trim().is_empty() || value.len() > max {
        Err(error)
    } else {
        Ok(())
    }
}
fn valid_diagnostic(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= MAX_OUTBOX_DIAGNOSTIC_BYTES
}
fn valid_persisted_claim(claim: &DeliveryClaim) -> bool {
    !claim.owner.trim().is_empty()
        && claim.owner.len() <= MAX_OUTBOX_OWNER_BYTES
        && claim.fence > 0
        && claim.lease_until_ms != i64::MIN
}
fn valid_initial_claim(claim: &DeliveryClaim, last_fence: u64) -> bool {
    valid_persisted_claim(claim)
        && claim.heartbeat_revision == 0
        && last_fence.checked_add(1) == Some(claim.fence)
}
fn valid_renewal(current: &DeliveryClaim, renewed: &DeliveryClaim) -> bool {
    renewed.owner == current.owner
        && renewed.fence == current.fence
        && current.heartbeat_revision.checked_add(1) == Some(renewed.heartbeat_revision)
        && renewed.lease_until_ms > current.lease_until_ms
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OutboxError {
    InvalidProcessId,
    InvalidKey,
    InvalidTopic,
    PayloadTooLarge,
    InvalidTransition,
    InvalidPersistedState,
    CounterInvariant,
    AttemptOverflow,
    RevisionOverflow,
}
impl fmt::Display for OutboxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "outbox error: {self:?}")
    }
}
impl std::error::Error for OutboxError {}

#[cfg(test)]
mod tests {
    use super::*;
    fn record() -> OutboxRecord {
        OutboxRecord::pending(
            OutboxId {
                process_id: "p".into(),
                key: "event:1".into(),
            },
            "task.completed".into(),
            "{}".into(),
            0,
        )
        .unwrap()
    }
    fn claim(owner: &str, fence: u64) -> DeliveryClaim {
        DeliveryClaim {
            owner: owner.into(),
            fence,
            lease_until_ms: 100,
            heartbeat_revision: 0,
        }
    }
    #[test]
    fn stale_dispatcher_cannot_confirm_new_claim() {
        let first = claim("a", 1);
        let claimed = record()
            .evolve(OutboxEvent::Claim {
                claim: first.clone(),
                now_ms: 0,
            })
            .unwrap();
        let released = claimed
            .evolve(OutboxEvent::ReleaseExpiredClaim { now_ms: 100 })
            .unwrap();
        let second = DeliveryClaim {
            owner: "b".into(),
            fence: 2,
            lease_until_ms: 200,
            heartbeat_revision: 0,
        };
        let reclaimed = released
            .evolve(OutboxEvent::Claim {
                claim: second.clone(),
                now_ms: 100,
            })
            .unwrap();
        assert_eq!(reclaimed.attempt, 2);
        assert_eq!(reclaimed.last_fence, 2);
        assert_eq!(
            reclaimed.evolve(OutboxEvent::ConfirmDelivered {
                claim: first,
                receipt: "old".into(),
                now_ms: 0
            }),
            Err(OutboxError::InvalidTransition)
        );
        assert!(matches!(
            reclaimed
                .evolve(OutboxEvent::ConfirmDelivered {
                    claim: second,
                    receipt: "ok".into(),
                    now_ms: 0
                })
                .unwrap()
                .state,
            OutboxState::Delivered { .. }
        ));
    }
    #[test]
    fn fence_cannot_be_reused_or_skipped_after_release() {
        let first = claim("a", 1);
        let released = record()
            .evolve(OutboxEvent::Claim {
                claim: first,
                now_ms: 0,
            })
            .unwrap()
            .evolve(OutboxEvent::ReleaseExpiredClaim { now_ms: 100 })
            .unwrap();
        for fence in [1, 3] {
            assert_eq!(
                released.evolve(OutboxEvent::Claim {
                    claim: claim("b", fence),
                    now_ms: 100
                }),
                Err(OutboxError::InvalidTransition)
            );
        }
        let next = DeliveryClaim {
            owner: "b".into(),
            fence: 2,
            lease_until_ms: 200,
            heartbeat_revision: 0,
        };
        assert!(
            released
                .evolve(OutboxEvent::Claim {
                    claim: next,
                    now_ms: 100
                })
                .is_ok()
        );
    }
    #[test]
    fn unknown_delivery_is_not_automatically_dispatchable() {
        let claim = claim("a", 1);
        let unknown = record()
            .evolve(OutboxEvent::Claim {
                claim: claim.clone(),
                now_ms: 0,
            })
            .unwrap()
            .evolve(OutboxEvent::MarkDeliveryUnknown {
                claim,
                reason: "connection lost after send".into(),
                now_ms: 0,
            })
            .unwrap();
        assert!(!unknown.state.is_dispatchable(i64::MAX));
        assert!(!unknown.state.is_terminal());
    }
    #[test]
    fn retry_time_is_application_selected_and_quarantine_is_terminal() {
        let claim = claim("a", 1);
        let retry = record()
            .evolve(OutboxEvent::Claim {
                claim: claim.clone(),
                now_ms: 0,
            })
            .unwrap()
            .evolve(OutboxEvent::Reschedule {
                claim,
                available_at_ms: 100,
                error: "temporary".into(),
                now_ms: 0,
            })
            .unwrap();
        assert!(!retry.state.is_dispatchable(99));
        assert!(retry.state.is_dispatchable(100));
        let quarantined = retry
            .evolve(OutboxEvent::Quarantine {
                reason: "operator review".into(),
            })
            .unwrap();
        assert!(quarantined.state.is_terminal());
    }
    #[test]
    fn publication_rejects_oversized_payload_before_state_exists() {
        let error = OutboxRecord::pending(
            OutboxId {
                process_id: "p".into(),
                key: "k".into(),
            },
            "topic".into(),
            "x".repeat(MAX_OUTBOX_PAYLOAD_BYTES + 1),
            0,
        )
        .unwrap_err();
        assert_eq!(error, OutboxError::PayloadTooLarge);
    }
    #[test]
    fn renewal_requires_same_fence_and_monotonic_heartbeat() {
        let current = claim("a", 1);
        let claimed = record()
            .evolve(OutboxEvent::Claim {
                claim: current.clone(),
                now_ms: 0,
            })
            .unwrap();
        let invalid = DeliveryClaim {
            owner: "a".into(),
            fence: 2,
            lease_until_ms: 200,
            heartbeat_revision: 1,
        };
        assert_eq!(
            claimed.evolve(OutboxEvent::RenewClaim {
                current,
                renewed: invalid,
                now_ms: 50
            }),
            Err(OutboxError::InvalidTransition)
        );
    }
    #[test]
    fn validation_rejects_impossible_counter_combinations() {
        let mut invalid = record();
        invalid.attempt = 1;
        assert_eq!(invalid.validate(), Err(OutboxError::CounterInvariant));

        let mut invalid = record();
        invalid.last_fence = 1;
        assert_eq!(invalid.validate(), Err(OutboxError::CounterInvariant));

        let mut invalid = record();
        invalid.attempt = 1;
        invalid.last_fence = 1;
        invalid.revision = 0;
        assert_eq!(invalid.validate(), Err(OutboxError::CounterInvariant));
    }

    #[test]
    fn validation_rejects_claimed_or_delivered_without_a_real_attempt() {
        let mut claimed = record();
        claimed.state = OutboxState::Claimed(claim("a", 1));
        assert_eq!(claimed.validate(), Err(OutboxError::InvalidPersistedState));

        let mut delivered = record();
        delivered.state = OutboxState::Delivered {
            receipt: "impossible".into(),
        };
        assert_eq!(
            delivered.validate(),
            Err(OutboxError::InvalidPersistedState)
        );
    }

    #[test]
    fn every_reachable_state_remains_valid() {
        let initial = record();
        initial.validate().unwrap();
        let claim = claim("a", 1);
        let claimed = initial
            .evolve(OutboxEvent::Claim {
                claim: claim.clone(),
                now_ms: 0,
            })
            .unwrap();
        claimed.validate().unwrap();
        let unknown = claimed
            .evolve(OutboxEvent::MarkDeliveryUnknown {
                claim,
                reason: "uncertain".into(),
                now_ms: 0,
            })
            .unwrap();
        unknown.validate().unwrap();
        unknown
            .evolve(OutboxEvent::Quarantine {
                reason: "reviewed".into(),
            })
            .unwrap()
            .validate()
            .unwrap();
    }

    #[test]
    fn validation_rejects_impossible_persisted_state_combinations() {
        let mut malformed = record();
        malformed.attempt = 1;
        assert_eq!(malformed.validate(), Err(OutboxError::CounterInvariant));

        let mut malformed = record();
        malformed.state = OutboxState::Claimed(claim("worker", 1));
        assert_eq!(
            malformed.validate(),
            Err(OutboxError::InvalidPersistedState)
        );

        let mut malformed = record();
        malformed.state = OutboxState::DeliveryUnknown { reason: " ".into() };
        assert_eq!(
            malformed.validate(),
            Err(OutboxError::InvalidPersistedState)
        );
    }

    #[test]
    fn every_legal_transition_preserves_record_invariants() {
        let claimed = record()
            .evolve(OutboxEvent::Claim {
                claim: claim("a", 1),
                now_ms: 0,
            })
            .unwrap();
        assert_eq!(claimed.validate(), Ok(()));
        let renewed_claim = DeliveryClaim {
            owner: "a".into(),
            fence: 1,
            lease_until_ms: 200,
            heartbeat_revision: 1,
        };
        let renewed = claimed
            .evolve(OutboxEvent::RenewClaim {
                current: claim("a", 1),
                renewed: renewed_claim.clone(),
                now_ms: 50,
            })
            .unwrap();
        assert_eq!(renewed.validate(), Ok(()));
        let delivered = renewed
            .evolve(OutboxEvent::ConfirmDelivered {
                claim: renewed_claim,
                receipt: "ok".into(),
                now_ms: 0,
            })
            .unwrap();
        assert_eq!(delivered.validate(), Ok(()));
    }

    #[test]
    fn claim_and_renewal_time_boundary_matrix_is_strict() {
        let base = record();
        for (now_ms, lease_until_ms, accepted) in [
            (-1, 0, false),
            (0, 1, true),
            (99, 100, true),
            (100, 100, false),
            (101, 100, false),
        ] {
            let candidate = DeliveryClaim {
                owner: "worker".into(),
                fence: 1,
                lease_until_ms,
                heartbeat_revision: 0,
            };
            assert_eq!(
                base.evolve(OutboxEvent::Claim {
                    claim: candidate,
                    now_ms
                })
                .is_ok(),
                accepted,
                "claim boundary now={now_ms}, lease={lease_until_ms}",
            );
        }

        let current = claim("worker", 1);
        let claimed = base
            .evolve(OutboxEvent::Claim {
                claim: current.clone(),
                now_ms: 0,
            })
            .unwrap();
        for (now_ms, renewed_until_ms, accepted) in
            [(99, 101, true), (100, 101, false), (101, 200, false)]
        {
            let renewed = DeliveryClaim {
                owner: "worker".into(),
                fence: 1,
                lease_until_ms: renewed_until_ms,
                heartbeat_revision: 1,
            };
            assert_eq!(
                claimed
                    .evolve(OutboxEvent::RenewClaim {
                        current: current.clone(),
                        renewed,
                        now_ms,
                    })
                    .is_ok(),
                accepted,
                "renew boundary now={now_ms}, lease={renewed_until_ms}",
            );
        }
    }

    #[test]
    fn state_event_matrix_rejects_every_unlisted_transition() {
        let pending = record();
        let active_claim = claim("worker", 1);
        let claimed = pending
            .evolve(OutboxEvent::Claim {
                claim: active_claim.clone(),
                now_ms: 0,
            })
            .unwrap();
        let unknown = claimed
            .evolve(OutboxEvent::MarkDeliveryUnknown {
                claim: active_claim.clone(),
                reason: "uncertain".into(),
                now_ms: 0,
            })
            .unwrap();
        let delivered = claimed
            .evolve(OutboxEvent::ConfirmDelivered {
                claim: active_claim.clone(),
                receipt: "receipt".into(),
                now_ms: 0,
            })
            .unwrap();
        let quarantined = pending
            .evolve(OutboxEvent::Quarantine {
                reason: "operator".into(),
            })
            .unwrap();

        let samples = [pending, claimed, unknown, delivered, quarantined];
        for record in samples {
            let before = record.clone();
            let events = [
                OutboxEvent::Claim {
                    claim: active_claim.clone(),
                    now_ms: 0,
                },
                OutboxEvent::RenewClaim {
                    current: active_claim.clone(),
                    renewed: DeliveryClaim {
                        owner: "worker".into(),
                        fence: 1,
                        lease_until_ms: 200,
                        heartbeat_revision: 1,
                    },
                    now_ms: 50,
                },
                OutboxEvent::ConfirmDelivered {
                    claim: active_claim.clone(),
                    receipt: "receipt".into(),
                    now_ms: 0,
                },
                OutboxEvent::Reschedule {
                    claim: active_claim.clone(),
                    available_at_ms: 200,
                    error: "retry".into(),
                    now_ms: 0,
                },
                OutboxEvent::MarkDeliveryUnknown {
                    claim: active_claim.clone(),
                    reason: "unknown".into(),
                    now_ms: 0,
                },
                OutboxEvent::ReleaseExpiredClaim { now_ms: 100 },
                OutboxEvent::Quarantine {
                    reason: "operator".into(),
                },
            ];
            for event in events {
                if let Ok(next) = record.evolve(event) {
                    next.validate().unwrap();
                    assert_eq!(next.id, before.id);
                    assert_eq!(next.topic, before.topic);
                    assert_eq!(next.payload, before.payload);
                    assert_eq!(next.revision, before.revision + 1);
                }
                assert_eq!(record, before, "evolve must be side-effect free");
            }
        }
    }

    #[test]
    fn textual_resource_boundary_matrix_uses_utf8_bytes() {
        let valid_ascii = "x".repeat(MAX_OUTBOX_TOPIC_BYTES);
        assert!(
            OutboxRecord::pending(
                OutboxId {
                    process_id: "p".into(),
                    key: "k".into()
                },
                valid_ascii,
                String::new(),
                0,
            )
            .is_ok()
        );

        let oversized_ascii = "x".repeat(MAX_OUTBOX_TOPIC_BYTES + 1);
        assert_eq!(
            OutboxRecord::pending(
                OutboxId {
                    process_id: "p".into(),
                    key: "k".into()
                },
                oversized_ascii,
                String::new(),
                0,
            )
            .unwrap_err(),
            OutboxError::InvalidTopic,
        );

        let multibyte = "界".repeat(MAX_OUTBOX_TOPIC_BYTES / 3 + 1);
        assert!(multibyte.chars().count() < MAX_OUTBOX_TOPIC_BYTES);
        assert!(multibyte.len() > MAX_OUTBOX_TOPIC_BYTES);
        assert_eq!(
            OutboxRecord::pending(
                OutboxId {
                    process_id: "p".into(),
                    key: "k".into()
                },
                multibyte,
                String::new(),
                0,
            )
            .unwrap_err(),
            OutboxError::InvalidTopic,
        );
    }

    #[test]
    fn identity_and_owner_resource_limits_are_enforced() {
        for (id, error) in [
            (
                OutboxId {
                    process_id: "p".repeat(MAX_OUTBOX_PROCESS_ID_BYTES + 1),
                    key: "k".into(),
                },
                OutboxError::InvalidProcessId,
            ),
            (
                OutboxId {
                    process_id: "p".into(),
                    key: "k".repeat(MAX_OUTBOX_KEY_BYTES + 1),
                },
                OutboxError::InvalidKey,
            ),
        ] {
            assert_eq!(
                OutboxRecord::pending(id, "topic".into(), String::new(), 0).unwrap_err(),
                error,
            );
        }

        let oversized_owner = DeliveryClaim {
            owner: "w".repeat(MAX_OUTBOX_OWNER_BYTES + 1),
            fence: 1,
            lease_until_ms: 1,
            heartbeat_revision: 0,
        };
        assert_eq!(
            record().evolve(OutboxEvent::Claim {
                claim: oversized_owner,
                now_ms: 0
            }),
            Err(OutboxError::InvalidTransition),
        );
    }

    #[test]
    fn integer_extreme_matrix_never_wraps_or_panics() {
        let mut attempt_max = record();
        attempt_max.attempt = u32::MAX;
        attempt_max.last_fence = u64::from(u32::MAX);
        attempt_max.revision = u64::from(u32::MAX);
        assert_eq!(
            attempt_max.evolve(OutboxEvent::Claim {
                claim: DeliveryClaim {
                    owner: "w".into(),
                    fence: u64::from(u32::MAX) + 1,
                    lease_until_ms: 1,
                    heartbeat_revision: 0,
                },
                now_ms: 0,
            }),
            Err(OutboxError::AttemptOverflow),
        );

        let mut revision_max = record();
        revision_max.revision = u64::MAX;
        assert_eq!(
            revision_max.evolve(OutboxEvent::Quarantine {
                reason: "stop".into()
            }),
            Err(OutboxError::RevisionOverflow),
        );

        let current = DeliveryClaim {
            owner: "w".into(),
            fence: 1,
            lease_until_ms: i64::MAX,
            heartbeat_revision: u64::MAX,
        };
        let mut claimed = record();
        claimed.state = OutboxState::Claimed(current.clone());
        claimed.attempt = 1;
        claimed.last_fence = 1;
        claimed.revision = 1;
        assert_eq!(claimed.validate(), Ok(()));
        assert_eq!(
            claimed.evolve(OutboxEvent::RenewClaim {
                current: current.clone(),
                renewed: current,
                now_ms: i64::MAX - 1,
            }),
            Err(OutboxError::InvalidTransition),
        );
    }

    #[test]
    fn terminal_states_are_closed_under_all_events() {
        let claim = claim("worker", 1);
        let claimed = record()
            .evolve(OutboxEvent::Claim {
                claim: claim.clone(),
                now_ms: 0,
            })
            .unwrap();
        let terminal = [
            claimed
                .evolve(OutboxEvent::ConfirmDelivered {
                    claim: claim.clone(),
                    receipt: "ok".into(),
                    now_ms: 0,
                })
                .unwrap(),
            record()
                .evolve(OutboxEvent::Quarantine {
                    reason: "stop".into(),
                })
                .unwrap(),
        ];
        for record in terminal {
            let before = record.clone();
            let events = [
                OutboxEvent::Claim {
                    claim: claim.clone(),
                    now_ms: 0,
                },
                OutboxEvent::RenewClaim {
                    current: claim.clone(),
                    renewed: DeliveryClaim {
                        owner: "worker".into(),
                        fence: 1,
                        lease_until_ms: 200,
                        heartbeat_revision: 1,
                    },
                    now_ms: 50,
                },
                OutboxEvent::ConfirmDelivered {
                    claim: claim.clone(),
                    receipt: "again".into(),
                    now_ms: 0,
                },
                OutboxEvent::Reschedule {
                    claim: claim.clone(),
                    available_at_ms: 200,
                    error: "retry".into(),
                    now_ms: 0,
                },
                OutboxEvent::MarkDeliveryUnknown {
                    claim: claim.clone(),
                    reason: "unknown".into(),
                    now_ms: 0,
                },
                OutboxEvent::ReleaseExpiredClaim { now_ms: 100 },
                OutboxEvent::Quarantine {
                    reason: "again".into(),
                },
            ];
            for event in events {
                assert_eq!(record.evolve(event), Err(OutboxError::InvalidTransition));
                assert_eq!(record, before);
            }
        }
    }

    fn audit_events(record: &OutboxRecord) -> Vec<OutboxEvent> {
        let next_fence = record.last_fence.saturating_add(1);
        let fresh = DeliveryClaim {
            owner: "audit-worker".into(),
            fence: next_fence,
            lease_until_ms: 10,
            heartbeat_revision: 0,
        };
        let current = match &record.state {
            OutboxState::Claimed(claim) => claim.clone(),
            _ => fresh.clone(),
        };
        let renewed = DeliveryClaim {
            owner: current.owner.clone(),
            fence: current.fence,
            lease_until_ms: current.lease_until_ms.saturating_add(1),
            heartbeat_revision: current.heartbeat_revision.saturating_add(1),
        };
        vec![
            OutboxEvent::Claim {
                claim: fresh,
                now_ms: i64::MIN,
            },
            OutboxEvent::Claim {
                claim: DeliveryClaim {
                    owner: "audit-worker".into(),
                    fence: next_fence,
                    lease_until_ms: 101,
                    heartbeat_revision: 0,
                },
                now_ms: 100,
            },
            OutboxEvent::RenewClaim {
                current: current.clone(),
                renewed,
                now_ms: current.lease_until_ms.saturating_sub(1),
            },
            OutboxEvent::ConfirmDelivered {
                claim: current.clone(),
                receipt: "receipt".into(),
                now_ms: 0,
            },
            OutboxEvent::Reschedule {
                claim: current.clone(),
                available_at_ms: 20,
                error: "retry".into(),
                now_ms: 0,
            },
            OutboxEvent::MarkDeliveryUnknown {
                claim: current,
                reason: "unknown".into(),
                now_ms: 0,
            },
            OutboxEvent::ReleaseExpiredClaim { now_ms: i64::MIN },
            OutboxEvent::ReleaseExpiredClaim { now_ms: i64::MAX },
            OutboxEvent::Quarantine {
                reason: "audit".into(),
            },
        ]
    }

    #[test]
    fn bounded_state_space_audit_is_deterministic_and_invariant_preserving() {
        let initial = record();
        let mut frontier = vec![initial.clone()];
        let mut reached = vec![initial];
        for _depth in 0..6 {
            let mut next = Vec::new();
            for source in frontier {
                assert_eq!(source.validate(), Ok(()));
                for event in audit_events(&source) {
                    let before = source.clone();
                    let first = source.evolve(event.clone());
                    let second = source.evolve(event);
                    assert_eq!(first, second, "same state and event must be deterministic");
                    assert_eq!(source, before, "evolve must not mutate its source");
                    if let Ok(target) = first {
                        assert_eq!(target.validate(), Ok(()));
                        assert_eq!(target.revision, source.revision + 1);
                        assert_eq!(target.id, source.id);
                        assert_eq!(target.topic, source.topic);
                        assert_eq!(target.payload, source.payload);
                        if !reached.contains(&target) {
                            reached.push(target.clone());
                            next.push(target);
                        }
                    }
                }
            }
            frontier = next;
            if frontier.is_empty() {
                break;
            }
        }
        assert!(
            reached
                .iter()
                .any(|record| matches!(record.state, OutboxState::Delivered { .. }))
        );
        assert!(
            reached
                .iter()
                .any(|record| matches!(record.state, OutboxState::DeliveryUnknown { .. }))
        );
        assert!(
            reached
                .iter()
                .any(|record| matches!(record.state, OutboxState::Quarantined { .. }))
        );
        assert!(reached.iter().any(|record| record.attempt >= 2));
    }

    #[test]
    fn invalid_events_leave_all_observable_fields_unchanged() {
        let source = record();
        let invalid = OutboxEvent::ConfirmDelivered {
            claim: claim("nobody", 1),
            receipt: "impossible".into(),
            now_ms: 0,
        };
        assert_eq!(source.evolve(invalid), Err(OutboxError::InvalidTransition));
        assert_eq!(source, record());
    }

    #[test]
    fn expired_claim_cannot_publish_any_external_outcome() {
        let active = claim("worker", 1);
        let claimed = record()
            .evolve(OutboxEvent::Claim {
                claim: active.clone(),
                now_ms: 0,
            })
            .unwrap();
        let expired_at = active.lease_until_ms;
        let outcomes = [
            OutboxEvent::ConfirmDelivered {
                claim: active.clone(),
                receipt: "late".into(),
                now_ms: expired_at,
            },
            OutboxEvent::Reschedule {
                claim: active.clone(),
                available_at_ms: expired_at + 1,
                error: "late retry".into(),
                now_ms: expired_at,
            },
            OutboxEvent::MarkDeliveryUnknown {
                claim: active,
                reason: "late unknown".into(),
                now_ms: expired_at,
            },
        ];
        for event in outcomes {
            assert_eq!(claimed.evolve(event), Err(OutboxError::InvalidTransition));
        }
    }
}
