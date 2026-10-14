use super::application::{OperationWakeSink, WakeError};
use super::commit::{CommitError, RecoveryCursor, RecoveryRepository};
use super::{OperationId, OperationRecord, OperationState};
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RecoveryDisposition {
    Wake,
    AwaitOwner,
    ReclaimExpired,
    FinalizeCommit,
    ManualReconciliation,
    IgnoreTerminal,
}

pub(crate) struct RecoveryPlanner;

impl RecoveryPlanner {
    pub(crate) fn classify(operation: &OperationRecord, now_ms: i64) -> RecoveryDisposition {
        match &operation.state {
            OperationState::Pending | OperationState::RetryScheduled { .. } => {
                RecoveryDisposition::Wake
            }
            OperationState::Claimed(claim) | OperationState::CommitPrepared(claim) => {
                if claim.is_expired(now_ms) {
                    RecoveryDisposition::ReclaimExpired
                } else {
                    RecoveryDisposition::AwaitOwner
                }
            }
            OperationState::Committed(_) => RecoveryDisposition::FinalizeCommit,
            OperationState::EffectUnknown { .. } => RecoveryDisposition::ManualReconciliation,
            OperationState::Quarantined { .. }
            | OperationState::Superseded { .. }
            | OperationState::Completed(_) => RecoveryDisposition::IgnoreTerminal,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct RecoveryReport {
    pub(crate) scanned: usize,
    pub(crate) woken: Vec<OperationId>,
    pub(crate) wake_failures: Vec<(OperationId, WakeError)>,
    pub(crate) awaiting_owner: Vec<OperationId>,
    pub(crate) finalized: Vec<OperationId>,
    pub(crate) reconciliation_required: Vec<OperationId>,
}

pub(crate) struct RecoveryService<'a, R, W> {
    repository: &'a mut R,
    wake_sink: &'a mut W,
}

impl<'a, R, W> RecoveryService<'a, R, W>
where
    R: RecoveryRepository,
    W: OperationWakeSink,
{
    pub(crate) fn new(repository: &'a mut R, wake_sink: &'a mut W) -> Self {
        Self {
            repository,
            wake_sink,
        }
    }

    pub(crate) fn scan(
        &mut self,
        now_ms: i64,
        page_size: usize,
    ) -> Result<RecoveryReport, RecoveryError> {
        let mut report = RecoveryReport::default();
        let mut cursor: Option<RecoveryCursor> = None;
        loop {
            let page = self
                .repository
                .recoverable_page(cursor.as_ref(), page_size)?;
            if page.operations.is_empty() {
                break;
            }
            report.scanned = report
                .scanned
                .checked_add(page.operations.len())
                .ok_or(RecoveryError::ScanCountOverflow)?;
            for operation in page.operations {
                match RecoveryPlanner::classify(&operation, now_ms) {
                    RecoveryDisposition::Wake => match self.wake_sink.wake(&operation.id) {
                        Ok(()) => report.woken.push(operation.id),
                        Err(error) => report.wake_failures.push((operation.id, error)),
                    },
                    RecoveryDisposition::AwaitOwner => report.awaiting_owner.push(operation.id),
                    RecoveryDisposition::ReclaimExpired => {
                        let reclaimed = self.repository.reclaim_expired_operation(
                            &operation.id,
                            operation.revision,
                            now_ms,
                        )?;
                        match reclaimed.state {
                            OperationState::RetryScheduled { .. } => {
                                match self.wake_sink.wake(&reclaimed.id) {
                                    Ok(()) => report.woken.push(reclaimed.id),
                                    Err(error) => report.wake_failures.push((reclaimed.id, error)),
                                }
                            }
                            OperationState::EffectUnknown { .. } => {
                                report.reconciliation_required.push(reclaimed.id)
                            }
                            _ => unreachable!(
                                "expired claim reclamation must classify effect safety"
                            ),
                        }
                    }
                    RecoveryDisposition::FinalizeCommit => {
                        let commit = match operation.state {
                            OperationState::Committed(commit) => commit,
                            _ => unreachable!(),
                        };
                        let finalized = self.repository.finalize_committed_operation(
                            &operation.id,
                            operation.revision,
                            commit,
                        )?;
                        report.finalized.push(finalized.id);
                    }
                    RecoveryDisposition::ManualReconciliation => {
                        report.reconciliation_required.push(operation.id)
                    }
                    RecoveryDisposition::IgnoreTerminal => {}
                }
            }
            match page.next {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        Ok(report)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RecoveryError {
    Repository(CommitError),
    ScanCountOverflow,
}
impl From<CommitError> for RecoveryError {
    fn from(error: CommitError) -> Self {
        Self::Repository(error)
    }
}
impl fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "recovery error: {self:?}")
    }
}
impl std::error::Error for RecoveryError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::commit::{AggregateRepository, MemoryAggregateRepositoryAccess};
    use crate::kernel::{
        AggregateSnapshot, AggregateVersion, ClaimToken, CommitToken, MemoryAggregateRepository,
        OperationEvent, OperationKind,
    };
    use std::collections::{BTreeMap, HashMap};

    #[derive(Default)]
    struct WakeSink {
        ids: Vec<OperationId>,
        fail_sequence: Option<u64>,
    }
    impl OperationWakeSink for WakeSink {
        fn wake(&mut self, id: &OperationId) -> Result<(), WakeError> {
            if self.fail_sequence == Some(id.sequence) {
                Err(WakeError("offline".into()))
            } else {
                self.ids.push(id.clone());
                Ok(())
            }
        }
    }
    fn id(sequence: u64) -> OperationId {
        OperationId {
            process_id: "p".into(),
            task_id: "t".into(),
            generation: 0,
            sequence,
        }
    }
    fn repository() -> MemoryAggregateRepository {
        let claim = ClaimToken {
            owner: "r".into(),
            fence: 1,
            lease_until_ms: i64::MAX,
            heartbeat_revision: 0,
        };
        let commit = CommitToken {
            process_version: 1,
            digest: "d".into(),
        };
        let pending = OperationRecord::pending(id(0), OperationKind::ExecuteTask);
        let retry = OperationRecord::pending(id(1), OperationKind::ExecuteTask)
            .evolve(OperationEvent::Claim(claim.clone()))
            .unwrap()
            .evolve(OperationEvent::ScheduleRetry {
                claim: claim.clone(),
                attempt: 1,
            })
            .unwrap();
        let prepared = OperationRecord::pending(id(2), OperationKind::ExecuteTask)
            .evolve(OperationEvent::Claim(claim.clone()))
            .unwrap()
            .evolve(OperationEvent::PrepareCommit {
                claim: claim.clone(),
            })
            .unwrap();
        let committed = OperationRecord::pending(id(3), OperationKind::ExecuteTask)
            .evolve(OperationEvent::Claim(claim.clone()))
            .unwrap()
            .evolve(OperationEvent::PrepareCommit {
                claim: claim.clone(),
            })
            .unwrap()
            .evolve(OperationEvent::Commit {
                claim: claim.clone(),
                commit,
            })
            .unwrap();
        let unknown = OperationRecord::pending(id(4), OperationKind::ExecuteTask)
            .evolve(OperationEvent::Claim(claim.clone()))
            .unwrap()
            .evolve(OperationEvent::MarkEffectUnknown { claim, attempt: 1 })
            .unwrap();
        let snapshot = AggregateSnapshot {
            process_id: "p".into(),
            version: AggregateVersion {
                graph: 0,
                process: 0,
                operations: 0,
            },
            executions: HashMap::new(),
            variables: BTreeMap::new(),
            operations: HashMap::from([
                (pending.id.clone(), pending),
                (retry.id.clone(), retry),
                (prepared.id.clone(), prepared),
                (committed.id.clone(), committed),
                (unknown.id.clone(), unknown),
            ]),
            outbox: HashMap::new(),
        };
        let mut repository = MemoryAggregateRepository::default();
        repository.insert_for_test(snapshot).unwrap();
        repository
    }

    #[test]
    fn recovery_classifies_without_guessing_from_elapsed_time() {
        let mut repository = repository();
        let mut sink = WakeSink::default();
        let report = RecoveryService::new(&mut repository, &mut sink)
            .scan(100, 2)
            .unwrap();
        assert_eq!(report.scanned, 5);
        assert_eq!(report.woken, [id(0), id(1)]);
        assert_eq!(report.awaiting_owner, [id(2)]);
        assert_eq!(report.finalized, [id(3)]);
        assert_eq!(report.reconciliation_required, [id(4)]);
        let snapshot = repository.load("p").unwrap().unwrap();
        assert!(matches!(
            snapshot.operations[&id(3)].state,
            OperationState::Completed(_)
        ));
    }

    #[test]
    fn wake_failure_remains_reported_and_durable() {
        let mut repository = repository();
        let mut sink = WakeSink {
            fail_sequence: Some(1),
            ..WakeSink::default()
        };
        let report = RecoveryService::new(&mut repository, &mut sink)
            .scan(100, 2)
            .unwrap();
        assert_eq!(report.woken, [id(0)]);
        assert_eq!(report.wake_failures.len(), 1);
        assert_eq!(report.wake_failures[0].0, id(1));
        assert_eq!(
            repository
                .recoverable_page(None, 10)
                .unwrap()
                .operations
                .len(),
            4
        );
    }
    #[test]
    fn expired_owner_is_fenced_released_and_rewoken() {
        let repository = repository();
        let claim = ClaimToken {
            owner: "dead-runtime".into(),
            fence: 2,
            lease_until_ms: 10,
            heartbeat_revision: 0,
        };
        let expired = OperationRecord::pending(id(5), OperationKind::ExecuteTask)
            .evolve(OperationEvent::Claim(claim))
            .unwrap();
        let mut snapshot = repository.load("p").unwrap().unwrap();
        snapshot.operations.insert(expired.id.clone(), expired);
        let mut replacement = MemoryAggregateRepository::default();
        replacement.insert_for_test(snapshot).unwrap();
        let mut sink = WakeSink::default();
        let report = RecoveryService::new(&mut replacement, &mut sink)
            .scan(10, 2)
            .unwrap();
        assert!(report.woken.contains(&id(5)));
        let recovered = replacement.load("p").unwrap().unwrap();
        assert_eq!(
            recovered.operations[&id(5)].state,
            OperationState::RetryScheduled { attempt: 2 }
        );
    }
    #[test]
    fn expired_prepared_owner_is_reconciled_and_never_rewoken() {
        let repository = repository();
        let claim = ClaimToken {
            owner: "dead-runtime".into(),
            fence: 6,
            lease_until_ms: 10,
            heartbeat_revision: 0,
        };
        let prepared = OperationRecord::pending(id(6), OperationKind::ExecuteTask)
            .evolve(OperationEvent::Claim(claim.clone()))
            .unwrap()
            .evolve(OperationEvent::PrepareCommit { claim })
            .unwrap();
        let mut snapshot = repository.load("p").unwrap().unwrap();
        snapshot.operations.insert(prepared.id.clone(), prepared);
        let mut replacement = MemoryAggregateRepository::default();
        replacement.insert_for_test(snapshot).unwrap();
        let mut sink = WakeSink::default();
        let report = RecoveryService::new(&mut replacement, &mut sink)
            .scan(10, 10)
            .unwrap();
        assert!(report.reconciliation_required.contains(&id(6)));
        assert!(!report.woken.contains(&id(6)));
        let recovered = replacement.load("p").unwrap().unwrap();
        assert_eq!(
            recovered.operations[&id(6)].state,
            OperationState::EffectUnknown { attempt: 6 }
        );
    }
}
