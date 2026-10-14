use super::command::{ActionPlanner, ApplyActionCommand};
use super::commit::{
    AggregateRepository, CommitError, CommitReceipt, MemoryAggregateRepository, OperationRepository,
};
use super::{ClaimToken, OperationId, OperationState};
use std::fmt;

/// Scheduler boundary. A wake-up is only an optimization over already durable
/// operations. Losing it is safe because recovery scans the same repository.
pub(crate) trait OperationWakeSink {
    fn wake(&mut self, operation: &OperationId) -> Result<(), WakeError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WakeError(pub(crate) String);

/// One vertical application service for action execution. It owns orchestration
/// only; validation and state decisions remain in pure planners, while every
/// ownership/state transition remains in the repository.
pub(crate) struct ActionApplicationService<R, W> {
    repository: R,
    wake_sink: W,
}

impl<R, W> ActionApplicationService<R, W>
where
    R: AggregateRepository + OperationRepository,
    W: OperationWakeSink,
{
    pub(crate) fn new(repository: R, wake_sink: W) -> Self {
        Self {
            repository,
            wake_sink,
        }
    }

    pub(crate) fn execute(
        &mut self,
        mut command: ApplyActionCommand,
        expected_operation_revision: u64,
        claim: ClaimToken,
    ) -> Result<ActionExecution, ApplicationError> {
        let operation_id = command.source_operation.id.clone();
        ActionPlanner::validate_command(&command)
            .map_err(|error| ApplicationError::Planning(error.to_string()))?;
        if command.source_operation.claim != claim {
            return Err(ApplicationError::ClaimEnvelopeMismatch);
        }

        // Full authoritative preflight happens before ownership. Semantic and
        // caller-correctable failures leave the operation Pending.
        let preflight_snapshot = self
            .repository
            .load(&command.process_id)?
            .ok_or_else(|| ApplicationError::AggregateNotFound(command.process_id.clone()))?;
        ActionPlanner::plan(&preflight_snapshot, &command)
            .map_err(|error| ApplicationError::Planning(error.to_string()))?;

        let claimed = self.repository.claim_operation(
            &operation_id,
            expected_operation_revision,
            claim.clone(),
        )?;
        let prepared =
            self.repository
                .prepare_operation(&operation_id, claimed.revision, claim.clone())?;
        debug_assert_eq!(
            prepared.state,
            OperationState::CommitPrepared(claim.clone())
        );

        // Re-read after prepare. The aggregate operation version changed, so a
        // plan formed before ownership became durable would be stale by design.
        let snapshot = self
            .repository
            .load(&command.process_id)?
            .ok_or_else(|| ApplicationError::AggregateNotFound(command.process_id.clone()))?;
        command.source_operation.claim = claim.clone();
        let plan = match ActionPlanner::plan(&snapshot, &command) {
            Ok(plan) => plan,
            Err(error) => {
                let attempt =
                    u32::try_from(claim.fence).map_err(|_| ApplicationError::AttemptOverflow)?;
                self.repository.schedule_operation_retry(
                    &operation_id,
                    prepared.revision,
                    claim.clone(),
                    attempt,
                )?;
                return Err(ApplicationError::AuthoritativeStateChanged(
                    error.to_string(),
                ));
            }
        };
        let receipt = self.repository.compare_and_commit(&plan)?;

        let committed = self
            .repository
            .load(&command.process_id)?
            .and_then(|snapshot| snapshot.operations.get(&operation_id).cloned())
            .ok_or_else(|| ApplicationError::OperationLost(operation_id.clone()))?;
        let completed = self.repository.complete_operation(
            &operation_id,
            committed.revision,
            receipt.token.clone(),
        )?;

        let mut wake_failures = Vec::new();
        for operation in &receipt.new_operations {
            if let Err(error) = self.wake_sink.wake(operation) {
                // The operation is already durable. Report degraded delivery
                // without rolling back committed business state.
                wake_failures.push((operation.clone(), error));
            }
        }
        Ok(ActionExecution {
            receipt,
            completed_revision: completed.revision,
            wake_failures,
        })
    }

    pub(crate) fn into_parts(self) -> (R, W) {
        (self.repository, self.wake_sink)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ActionExecution {
    pub(crate) receipt: CommitReceipt,
    pub(crate) completed_revision: u64,
    pub(crate) wake_failures: Vec<(OperationId, WakeError)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ApplicationError {
    ClaimEnvelopeMismatch,
    AggregateNotFound(String),
    OperationLost(OperationId),
    Planning(String),
    AuthoritativeStateChanged(String),
    AttemptOverflow,
    Repository(CommitError),
}

impl From<CommitError> for ApplicationError {
    fn from(error: CommitError) -> Self {
        Self::Repository(error)
    }
}
impl fmt::Display for ApplicationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "application error: {self:?}")
    }
}
impl std::error::Error for ApplicationError {}

/// Explicit alias used by the first adapter migration. Keeping it here makes
/// the required repository semantics visible at the composition boundary.
pub(crate) type MemoryActionApplicationService<W> =
    ActionApplicationService<MemoryAggregateRepository, W>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::{
        ActionKind, AggregateSnapshot, AggregateVersion, ExecutionId, ExecutionSnapshot,
        ExecutionState, NodeId, OperationCommit, OperationKind, OperationRecord,
    };
    use std::collections::{BTreeMap, HashMap};

    #[derive(Default)]
    struct RecordingWakeSink {
        woken: Vec<OperationId>,
        fail: bool,
    }
    impl OperationWakeSink for RecordingWakeSink {
        fn wake(&mut self, operation: &OperationId) -> Result<(), WakeError> {
            if self.fail {
                Err(WakeError("queue unavailable".into()))
            } else {
                self.woken.push(operation.clone());
                Ok(())
            }
        }
    }

    fn execution() -> ExecutionId {
        ExecutionId::new("step", 0).unwrap()
    }
    fn source_id() -> OperationId {
        OperationId {
            process_id: "p1".into(),
            task_id: "step".into(),
            generation: 0,
            sequence: 0,
        }
    }
    fn claim() -> ClaimToken {
        ClaimToken {
            owner: "runtime-a".into(),
            fence: 7,
            lease_until_ms: i64::MAX,
            heartbeat_revision: 0,
        }
    }
    fn service(fail_wake: bool) -> MemoryActionApplicationService<RecordingWakeSink> {
        service_with_state(fail_wake, ExecutionState::Waiting)
    }

    fn service_with_state(
        fail_wake: bool,
        state: ExecutionState,
    ) -> MemoryActionApplicationService<RecordingWakeSink> {
        let source = source_id();
        let snapshot = AggregateSnapshot {
            process_id: "p1".into(),
            version: AggregateVersion {
                graph: 0,
                process: 0,
                operations: 0,
            },
            executions: HashMap::from([(
                execution(),
                ExecutionSnapshot {
                    model_node: NodeId::parse("step").unwrap(),
                    parent: None,
                    predecessors: vec![],
                    state,
                    revision: 0,
                },
            )]),
            variables: BTreeMap::new(),
            operations: HashMap::from([(
                source.clone(),
                OperationRecord::pending(source, OperationKind::ApplyAction),
            )]),
            outbox: HashMap::new(),
        };
        let mut repository = MemoryAggregateRepository::default();
        repository.insert(snapshot).unwrap();
        ActionApplicationService::new(
            repository,
            RecordingWakeSink {
                fail: fail_wake,
                ..Default::default()
            },
        )
    }
    fn command() -> ApplyActionCommand {
        ApplyActionCommand {
            process_id: "p1".into(),
            execution: execution(),
            action: ActionKind::Complete,
            inputs: vec![("answer".into(), "42".into())],
            source_operation: OperationCommit {
                id: source_id(),
                claim: claim(),
            },
        }
    }

    #[test]
    fn vertical_action_flow_claims_plans_commits_completes_and_wakes() {
        let mut service = service(false);
        let result = service.execute(command(), 0, claim()).unwrap();
        assert_eq!(result.wake_failures, []);
        let (repository, wake) = service.into_parts();
        let snapshot = repository.load("p1").unwrap().unwrap();
        assert_eq!(
            snapshot.executions[&execution()].state,
            ExecutionState::Completed
        );
        assert_eq!(
            snapshot.operations[&source_id()].state,
            OperationState::Completed(result.receipt.token)
        );
        assert_eq!(wake.woken, result.receipt.new_operations);
    }

    #[test]
    fn lost_wakeup_does_not_roll_back_durable_commit() {
        let mut service = service(true);
        let result = service.execute(command(), 0, claim()).unwrap();
        assert_eq!(result.wake_failures.len(), 1);
        let (repository, _) = service.into_parts();
        let snapshot = repository.load("p1").unwrap().unwrap();
        assert_eq!(
            snapshot.executions[&execution()].state,
            ExecutionState::Completed
        );
        assert!(
            snapshot
                .operations
                .values()
                .any(|op| op.state == OperationState::Pending)
        );
    }

    #[test]
    fn stale_operation_revision_is_rejected_before_business_mutation() {
        let mut service = service(false);
        assert!(matches!(
            service.execute(command(), 9, claim()),
            Err(ApplicationError::Repository(
                CommitError::OperationRevisionConflict { .. }
            ))
        ));
        let (repository, _) = service.into_parts();
        assert_eq!(
            repository.load("p1").unwrap().unwrap().executions[&execution()].state,
            ExecutionState::Waiting
        );
    }
    #[test]
    fn invalid_user_input_is_rejected_before_operation_claim() {
        let mut service = service(false);
        let mut invalid = command();
        invalid.inputs.push(("answer".into(), "duplicate".into()));
        let error = service.execute(invalid, 0, claim()).unwrap_err();
        assert!(matches!(error, ApplicationError::Planning(_)));
        let (repository, wake) = service.into_parts();
        let snapshot = repository.load("p1").unwrap().unwrap();
        assert_eq!(
            snapshot.executions[&execution()].state,
            ExecutionState::Waiting
        );
        assert!(matches!(
            snapshot.operations[&source_id()].state,
            OperationState::Pending
        ));
        assert!(wake.woken.is_empty());
    }
    #[test]
    fn semantically_invalid_action_is_rejected_before_durable_ownership() {
        let mut service = service_with_state(false, ExecutionState::Completed);
        let error = service.execute(command(), 0, claim()).unwrap_err();
        assert!(matches!(error, ApplicationError::Planning(_)));
        let (repository, wake) = service.into_parts();
        let snapshot = repository.load("p1").unwrap().unwrap();
        assert_eq!(
            snapshot.operations[&source_id()].state,
            OperationState::Pending
        );
        assert!(wake.woken.is_empty());
    }
}
