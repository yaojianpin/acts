use super::clock::Clock;
use super::outbox::OutboxId;
use super::outbox_dispatch::{
    DispatchError, DispatchRequest, DispatchResult, OutboxTransport, dispatch_one,
};
use super::outbox_repository::{OutboxCursor, OutboxRepository, OutboxRepositoryError};

pub(crate) const MAX_DISPATCH_PAGE_SIZE: usize = 1024;
pub(crate) const MAX_DISPATCH_SCAN: usize = 100_000;
pub(crate) const MAX_DISPATCH_OWNER_BYTES: usize = 256;

/// Cooperative shutdown boundary. It is checked before every page and before
/// every external delivery, so shutdown latency is bounded by one in-flight
/// transport call rather than the whole batch.
pub(crate) trait StopSignal {
    fn is_stopped(&self) -> bool;
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NeverStop;
impl StopSignal for NeverStop {
    fn is_stopped(&self) -> bool {
        false
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DispatchBatchConfig {
    pub(crate) owner: String,
    pub(crate) lease_ms: i64,
    pub(crate) page_size: usize,
    pub(crate) max_records: usize,
}
impl DispatchBatchConfig {
    pub(crate) fn validate(&self) -> Result<(), DispatchBatchError> {
        if self.owner.trim().is_empty() || self.owner.len() > MAX_DISPATCH_OWNER_BYTES {
            return Err(DispatchBatchError::InvalidOwner);
        }
        if self.lease_ms <= 0 {
            return Err(DispatchBatchError::InvalidLease);
        }
        if self.page_size == 0 || self.page_size > MAX_DISPATCH_PAGE_SIZE {
            return Err(DispatchBatchError::InvalidPageSize);
        }
        if self.max_records == 0 || self.max_records > MAX_DISPATCH_SCAN {
            return Err(DispatchBatchError::InvalidScanBudget);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DispatchFailure {
    pub(crate) id: OutboxId,
    pub(crate) error: DispatchError,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct DispatchBatchReport {
    pub(crate) scanned: usize,
    pub(crate) delivered: Vec<OutboxId>,
    pub(crate) rescheduled: Vec<OutboxId>,
    pub(crate) outcome_unknown: Vec<OutboxId>,
    pub(crate) quarantined: Vec<OutboxId>,
    pub(crate) conflicts: Vec<OutboxId>,
    pub(crate) failures: Vec<DispatchFailure>,
    pub(crate) exhausted_budget: bool,
    pub(crate) stopped: bool,
}

/// Bounded, restart-safe outbox pass. The repository page is a snapshot of
/// candidates only: every delivery still uses revision CAS in `dispatch_one`.
pub(crate) fn dispatch_batch<R, T, C>(
    repository: &mut R,
    transport: &mut T,
    clock: &C,
    config: &DispatchBatchConfig,
) -> Result<DispatchBatchReport, DispatchBatchError>
where
    R: OutboxRepository,
    T: OutboxTransport,
    C: Clock,
{
    dispatch_batch_until(repository, transport, clock, config, &NeverStop)
}

pub(crate) fn dispatch_batch_until<R, T, C, S>(
    repository: &mut R,
    transport: &mut T,
    clock: &C,
    config: &DispatchBatchConfig,
    stop: &S,
) -> Result<DispatchBatchReport, DispatchBatchError>
where
    R: OutboxRepository,
    T: OutboxTransport,
    C: Clock,
    S: StopSignal,
{
    config.validate()?;
    // Freeze only the eligibility horizon for stable cursor pagination. A fresh
    // clock sample is taken immediately before each claim so slow transports do
    // not consume the lease budget of records later in the batch.
    let scan_at_ms = clock.now_ms();
    // Wall clocks can move backwards after NTP or VM correction. Never issue a
    // claim timestamp earlier than the eligibility snapshot or an earlier
    // claim in this pass. This preserves lease monotonicity without hiding
    // forward jumps, which must still affect expiry normally.
    let mut claim_time_floor_ms = scan_at_ms;
    let mut report = DispatchBatchReport::default();
    let mut cursor: Option<OutboxCursor> = None;

    while report.scanned < config.max_records {
        if stop.is_stopped() {
            report.stopped = true;
            break;
        }
        let remaining = config.max_records - report.scanned;
        let limit = config.page_size.min(remaining);
        let page = repository.dispatchable_outbox_page(scan_at_ms, cursor.as_ref(), limit)?;
        if page.records.is_empty() {
            break;
        }

        for record in page.records {
            if stop.is_stopped() {
                report.stopped = true;
                break;
            }
            report.scanned += 1;
            let id = record.id.clone();
            let claim_at_ms = clock.now_ms().max(claim_time_floor_ms);
            claim_time_floor_ms = claim_at_ms;
            let lease_until_ms = claim_at_ms
                .checked_add(config.lease_ms)
                .ok_or(DispatchBatchError::LeaseOverflow)?;
            let request = DispatchRequest {
                id: id.clone(),
                expected_revision: record.revision,
                owner: config.owner.clone(),
                lease_until_ms,
                now_ms: claim_at_ms,
            };
            match dispatch_one(repository, transport, request) {
                Ok(DispatchResult::Delivered(_)) => report.delivered.push(id),
                Ok(DispatchResult::Rescheduled(_)) => report.rescheduled.push(id),
                Ok(DispatchResult::OutcomeUnknown(_)) => report.outcome_unknown.push(id),
                Ok(DispatchResult::Quarantined(_)) => report.quarantined.push(id),
                Err(error) if is_conflict(&error) => report.conflicts.push(id),
                Err(error) => report.failures.push(DispatchFailure { id, error }),
            }
        }
        if report.stopped {
            break;
        }
        match page.next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    report.exhausted_budget = report.scanned == config.max_records;
    Ok(report)
}

fn is_conflict(error: &DispatchError) -> bool {
    matches!(
        error,
        DispatchError::StaleRequest { .. }
            | DispatchError::Repository(OutboxRepositoryError::RevisionConflict { .. })
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DispatchBatchError {
    InvalidOwner,
    InvalidLease,
    InvalidPageSize,
    InvalidScanBudget,
    LeaseOverflow,
    Repository(OutboxRepositoryError),
}
impl From<OutboxRepositoryError> for DispatchBatchError {
    fn from(value: OutboxRepositoryError) -> Self {
        Self::Repository(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::clock::FixedClock;
    use crate::kernel::outbox::{OutboxId, OutboxRecord};
    use crate::kernel::outbox_dispatch::DeliveryOutcome;
    use crate::kernel::outbox_repository::MemoryOutboxRepository;
    use std::collections::VecDeque;

    struct Transport {
        outcomes: VecDeque<DeliveryOutcome>,
    }
    impl OutboxTransport for Transport {
        fn deliver(&mut self, _: &OutboxRecord) -> DeliveryOutcome {
            self.outcomes.pop_front().expect("configured outcome")
        }
    }
    fn insert(repository: &mut MemoryOutboxRepository, key: &str) {
        let id = OutboxId {
            process_id: "p".into(),
            key: key.into(),
        };
        repository
            .insert(OutboxRecord::pending(id, "topic".into(), "{}".into(), 0).unwrap())
            .unwrap();
    }

    #[test]
    fn batch_is_bounded_and_reports_outcomes_without_losing_failed_records() {
        let mut repository = MemoryOutboxRepository::default();
        for key in ["1", "2", "3"] {
            insert(&mut repository, key);
        }
        let mut transport = Transport {
            outcomes: VecDeque::from([
                DeliveryOutcome::Delivered {
                    receipt: "r1".into(),
                },
                DeliveryOutcome::RetryAt {
                    available_at_ms: 500,
                    error: "later".into(),
                },
            ]),
        };
        let report = dispatch_batch(
            &mut repository,
            &mut transport,
            &FixedClock(100),
            &DispatchBatchConfig {
                owner: "worker".into(),
                lease_ms: 50,
                page_size: 1,
                max_records: 2,
            },
        )
        .unwrap();
        assert_eq!(report.scanned, 2);
        assert_eq!(report.delivered.len(), 1);
        assert_eq!(report.rescheduled.len(), 1);
        assert!(report.exhausted_budget);
        assert!(report.failures.is_empty());
        assert_eq!(
            repository
                .dispatchable_outbox_page(100, None, 10)
                .unwrap()
                .records
                .len(),
            1
        );
    }

    #[test]
    fn invalid_budget_is_rejected_before_repository_or_transport_use() {
        let mut repository = MemoryOutboxRepository::default();
        let mut transport = Transport {
            outcomes: VecDeque::new(),
        };
        let error = dispatch_batch(
            &mut repository,
            &mut transport,
            &FixedClock(0),
            &DispatchBatchConfig {
                owner: "worker".into(),
                lease_ms: 1,
                page_size: 0,
                max_records: 1,
            },
        )
        .unwrap_err();
        assert_eq!(error, DispatchBatchError::InvalidPageSize);
    }
    struct StopAfter(std::cell::Cell<usize>);
    impl StopSignal for StopAfter {
        fn is_stopped(&self) -> bool {
            let remaining = self.0.get();
            if remaining == 0 {
                true
            } else {
                self.0.set(remaining - 1);
                false
            }
        }
    }

    #[test]
    fn cooperative_stop_bounds_shutdown_to_before_next_delivery() {
        let mut repository = MemoryOutboxRepository::default();
        for key in ["1", "2", "3"] {
            insert(&mut repository, key);
        }
        let mut transport = Transport {
            outcomes: VecDeque::from([
                DeliveryOutcome::Delivered {
                    receipt: "r1".into(),
                },
                DeliveryOutcome::Delivered {
                    receipt: "r2".into(),
                },
                DeliveryOutcome::Delivered {
                    receipt: "r3".into(),
                },
            ]),
        };
        let report = dispatch_batch_until(
            &mut repository,
            &mut transport,
            &FixedClock(0),
            &DispatchBatchConfig {
                owner: "worker".into(),
                lease_ms: 10,
                page_size: 3,
                max_records: 3,
            },
            &StopAfter(std::cell::Cell::new(2)),
        )
        .unwrap();
        assert_eq!(report.scanned, 1);
        assert_eq!(report.delivered.len(), 1);
        assert!(report.stopped);
        assert!(!report.exhausted_budget);
        assert_eq!(
            repository
                .dispatchable_outbox_page(0, None, 10)
                .unwrap()
                .records
                .len(),
            2
        );
    }

    struct SequenceClock {
        values: Vec<i64>,
        index: std::sync::atomic::AtomicUsize,
    }
    impl Clock for SequenceClock {
        fn now_ms(&self) -> i64 {
            let index = self
                .index
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.values[index]
        }
    }
    struct ClaimRecordingTransport {
        leases: Vec<i64>,
    }
    impl OutboxTransport for ClaimRecordingTransport {
        fn deliver(&mut self, record: &OutboxRecord) -> DeliveryOutcome {
            let super::super::outbox::OutboxState::Claimed(claim) = &record.state else {
                panic!("transport must receive a claimed record");
            };
            self.leases.push(claim.lease_until_ms);
            DeliveryOutcome::Delivered {
                receipt: "ok".into(),
            }
        }
    }

    #[test]
    fn each_record_receives_a_fresh_full_lease_budget() {
        let mut repository = MemoryOutboxRepository::default();
        for key in ["1", "2"] {
            insert(&mut repository, key);
        }
        let clock = SequenceClock {
            values: vec![0, 10, 90],
            index: std::sync::atomic::AtomicUsize::new(0),
        };
        let mut transport = ClaimRecordingTransport { leases: Vec::new() };
        let report = dispatch_batch(
            &mut repository,
            &mut transport,
            &clock,
            &DispatchBatchConfig {
                owner: "worker".into(),
                lease_ms: 100,
                page_size: 2,
                max_records: 2,
            },
        )
        .unwrap();
        assert_eq!(report.delivered.len(), 2);
        assert_eq!(transport.leases, vec![110, 190]);
    }

    #[test]
    fn later_claim_clock_overflow_stops_before_external_delivery() {
        let mut repository = MemoryOutboxRepository::default();
        insert(&mut repository, "1");
        let clock = SequenceClock {
            values: vec![0, i64::MAX],
            index: std::sync::atomic::AtomicUsize::new(0),
        };
        let mut transport = ClaimRecordingTransport { leases: Vec::new() };
        let error = dispatch_batch(
            &mut repository,
            &mut transport,
            &clock,
            &DispatchBatchConfig {
                owner: "worker".into(),
                lease_ms: 1,
                page_size: 1,
                max_records: 1,
            },
        )
        .unwrap_err();
        assert_eq!(error, DispatchBatchError::LeaseOverflow);
        assert!(transport.leases.is_empty());
        assert_eq!(
            repository
                .dispatchable_outbox_page(0, None, 10)
                .unwrap()
                .records
                .len(),
            1
        );
    }

    #[test]
    fn backward_wall_clock_cannot_shorten_later_claim_leases() {
        let mut repository = MemoryOutboxRepository::default();
        for key in ["1", "2"] {
            insert(&mut repository, key);
        }
        let clock = SequenceClock {
            values: vec![100, 90, 80],
            index: std::sync::atomic::AtomicUsize::new(0),
        };
        let mut transport = ClaimRecordingTransport { leases: Vec::new() };
        let report = dispatch_batch(
            &mut repository,
            &mut transport,
            &clock,
            &DispatchBatchConfig {
                owner: "worker".into(),
                lease_ms: 50,
                page_size: 2,
                max_records: 2,
            },
        )
        .unwrap();
        assert_eq!(report.delivered.len(), 2);
        assert_eq!(transport.leases, vec![150, 150]);
    }

    #[test]
    fn oversized_owner_is_rejected_before_clock_repository_or_transport_use() {
        struct PanicClock;
        impl Clock for PanicClock {
            fn now_ms(&self) -> i64 {
                panic!("invalid config must not read the clock")
            }
        }
        let mut repository = MemoryOutboxRepository::default();
        let mut transport = Transport {
            outcomes: VecDeque::new(),
        };
        let error = dispatch_batch(
            &mut repository,
            &mut transport,
            &PanicClock,
            &DispatchBatchConfig {
                owner: "x".repeat(MAX_DISPATCH_OWNER_BYTES + 1),
                lease_ms: 1,
                page_size: 1,
                max_records: 1,
            },
        )
        .unwrap_err();
        assert_eq!(error, DispatchBatchError::InvalidOwner);
    }

    #[test]
    fn lease_overflow_happens_before_claim_or_transport() {
        struct PanicTransport;
        impl OutboxTransport for PanicTransport {
            fn deliver(&mut self, _: &OutboxRecord) -> DeliveryOutcome {
                panic!("overflow must be rejected before external delivery")
            }
        }
        let mut repository = MemoryOutboxRepository::default();
        insert(&mut repository, "1");
        let error = dispatch_batch(
            &mut repository,
            &mut PanicTransport,
            &FixedClock(i64::MAX),
            &DispatchBatchConfig {
                owner: "worker".into(),
                lease_ms: 1,
                page_size: 1,
                max_records: 1,
            },
        )
        .unwrap_err();
        assert_eq!(error, DispatchBatchError::LeaseOverflow);
        let id = OutboxId {
            process_id: "p".into(),
            key: "1".into(),
        };
        let saved = repository.load_outbox(&id).unwrap().unwrap();
        assert_eq!(saved.revision, 0);
        assert!(saved.state.is_dispatchable(i64::MAX));
    }
}
