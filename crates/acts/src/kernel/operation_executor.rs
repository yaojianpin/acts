use super::commit::{AggregateRepository, CommitError, OperationRepository};
use super::{ClaimToken, CommitToken, OperationId, OperationKind, OperationRecord, OperationState};

/// Facts reported by an operation handler. Policy is explicit and the executor
/// owns all durable lifecycle transitions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OperationOutcome {
    Committed(CommitToken),
    RetrySafe,
    EffectUnknown,
    Quarantine(QuarantineReason),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct QuarantineReason(String);

impl QuarantineReason {
    pub(crate) fn new(reason: impl Into<String>) -> Result<Self, InvalidQuarantineReason> {
        let reason = reason.into();
        if reason.trim().is_empty() {
            return Err(InvalidQuarantineReason);
        }
        Ok(Self(reason))
    }

    fn into_inner(self) -> String {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InvalidQuarantineReason;

pub(crate) trait OperationExecutionContext {
    fn operation(&self) -> &OperationRecord;
    fn claim(&self) -> &ClaimToken;
    fn renew_lease(&mut self, lease_until_ms: i64) -> Result<(), OperationExecutionError>;
}

pub(crate) trait OperationHandler {
    fn supports(&self, kind: OperationKind) -> bool;
    fn execute(&mut self, context: &mut dyn OperationExecutionContext) -> OperationOutcome;
}

struct DurableExecutionContext<'a, R> {
    repository: &'a mut R,
    operation: OperationRecord,
    claim: ClaimToken,
}

impl<R: OperationRepository> OperationExecutionContext for DurableExecutionContext<'_, R> {
    fn operation(&self) -> &OperationRecord {
        &self.operation
    }
    fn claim(&self) -> &ClaimToken {
        &self.claim
    }

    fn renew_lease(&mut self, lease_until_ms: i64) -> Result<(), OperationExecutionError> {
        if lease_until_ms <= self.claim.lease_until_ms {
            return Err(OperationExecutionError::LeaseNotExtended);
        }
        let heartbeat_revision = self
            .claim
            .heartbeat_revision
            .checked_add(1)
            .ok_or(OperationExecutionError::HeartbeatOverflow)?;
        let renewed = ClaimToken {
            owner: self.claim.owner.clone(),
            fence: self.claim.fence,
            lease_until_ms,
            heartbeat_revision,
        };
        self.operation = self.repository.renew_operation(
            &self.operation.id,
            self.operation.revision,
            self.claim.clone(),
            renewed.clone(),
        )?;
        self.claim = renewed;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OperationExecutionRequest {
    pub(crate) id: OperationId,
    pub(crate) expected_revision: u64,
    pub(crate) claim: ClaimToken,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OperationExecutionResult {
    Completed(OperationRecord),
    RetryScheduled(OperationRecord),
    EffectUnknown(OperationRecord),
    Quarantined(OperationRecord),
}

/// Uniform lifecycle for every operation kind. Handler outcomes explicitly
/// classify the external effect boundary; the executor never guesses.
pub(crate) fn execute_operation<R, H>(
    repository: &mut R,
    handler: &mut H,
    request: OperationExecutionRequest,
) -> Result<OperationExecutionResult, OperationExecutionError>
where
    R: AggregateRepository + OperationRepository,
    H: OperationHandler,
{
    validate_claim(&request.claim)?;
    let snapshot = repository
        .load(&request.id.process_id)?
        .ok_or_else(|| OperationExecutionError::NotFound(request.id.clone()))?;
    let current = snapshot
        .operations
        .get(&request.id)
        .cloned()
        .ok_or_else(|| OperationExecutionError::NotFound(request.id.clone()))?;
    if current.revision != request.expected_revision {
        return Err(OperationExecutionError::StaleRequest {
            expected: request.expected_revision,
            actual: current.revision,
        });
    }
    if !handler.supports(current.kind) {
        return Err(OperationExecutionError::UnsupportedKind(current.kind));
    }

    let claimed =
        repository.claim_operation(&request.id, current.revision, request.claim.clone())?;
    let prepared =
        repository.prepare_operation(&request.id, claimed.revision, request.claim.clone())?;

    let mut context = DurableExecutionContext {
        repository,
        operation: prepared,
        claim: request.claim,
    };
    let outcome = handler.execute(&mut context);
    apply_outcome(
        context.repository,
        &request.id,
        context.operation,
        context.claim,
        outcome,
    )
}

fn apply_outcome<R: OperationRepository>(
    repository: &mut R,
    id: &OperationId,
    prepared: OperationRecord,
    claim: ClaimToken,
    outcome: OperationOutcome,
) -> Result<OperationExecutionResult, OperationExecutionError> {
    match outcome {
        OperationOutcome::Committed(commit) => {
            let committed = repository.commit_operation(
                id,
                prepared.revision,
                claim.clone(),
                commit.clone(),
            )?;
            let completed = repository.complete_operation(id, committed.revision, commit)?;
            Ok(OperationExecutionResult::Completed(completed))
        }
        OperationOutcome::RetrySafe => {
            let attempt = attempt_from_claim(&claim)?;
            Ok(OperationExecutionResult::RetryScheduled(
                repository.schedule_operation_retry(
                    id,
                    prepared.revision,
                    claim.clone(),
                    attempt,
                )?,
            ))
        }
        OperationOutcome::EffectUnknown => {
            let attempt = attempt_from_claim(&claim)?;
            Ok(OperationExecutionResult::EffectUnknown(
                repository.mark_operation_effect_unknown(
                    id,
                    prepared.revision,
                    claim.clone(),
                    attempt,
                )?,
            ))
        }
        OperationOutcome::Quarantine(reason) => Ok(OperationExecutionResult::Quarantined(
            repository.quarantine_operation(
                id,
                prepared.revision,
                claim.clone(),
                reason.into_inner(),
            )?,
        )),
    }
}

fn attempt_from_claim(claim: &ClaimToken) -> Result<u32, OperationExecutionError> {
    u32::try_from(claim.fence).map_err(|_| OperationExecutionError::AttemptOverflow)
}

fn validate_claim(claim: &ClaimToken) -> Result<(), OperationExecutionError> {
    if claim.owner.trim().is_empty() {
        return Err(OperationExecutionError::InvalidOwner);
    }
    if claim.fence == 0 {
        return Err(OperationExecutionError::InvalidFence);
    }
    if u32::try_from(claim.fence).is_err() {
        return Err(OperationExecutionError::AttemptOverflow);
    }
    if claim.lease_until_ms == i64::MIN {
        return Err(OperationExecutionError::InvalidLease);
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OperationExecutionError {
    InvalidOwner,
    InvalidFence,
    InvalidLease,
    LeaseNotExtended,
    HeartbeatOverflow,
    AttemptOverflow,
    NotFound(OperationId),
    StaleRequest { expected: u64, actual: u64 },
    UnsupportedKind(OperationKind),
    Repository(CommitError),
}
impl From<CommitError> for OperationExecutionError {
    fn from(value: CommitError) -> Self {
        Self::Repository(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::{AggregateSnapshot, MemoryAggregateRepository};

    struct Handler {
        outcome: OperationOutcome,
        kind: OperationKind,
    }
    impl OperationHandler for Handler {
        fn supports(&self, kind: OperationKind) -> bool {
            kind == self.kind
        }
        fn execute(&mut self, _: &mut dyn OperationExecutionContext) -> OperationOutcome {
            self.outcome.clone()
        }
    }
    fn setup(kind: OperationKind) -> (MemoryAggregateRepository, OperationId) {
        let id = OperationId {
            process_id: "p".into(),
            task_id: "t".into(),
            generation: 0,
            sequence: 0,
        };
        let mut snapshot = AggregateSnapshot::empty("p").unwrap();
        snapshot
            .operations
            .insert(id.clone(), OperationRecord::pending(id.clone(), kind));
        let mut repository = MemoryAggregateRepository::default();
        repository.insert(snapshot).unwrap();
        (repository, id)
    }
    fn request(id: OperationId) -> OperationExecutionRequest {
        OperationExecutionRequest {
            id,
            expected_revision: 0,
            claim: ClaimToken {
                owner: "worker".into(),
                fence: 1,
                lease_until_ms: 100,
                heartbeat_revision: 0,
            },
        }
    }
    #[test]
    fn committed_outcome_runs_full_durable_lifecycle() {
        let (mut repository, id) = setup(OperationKind::ExecuteTask);
        let token = CommitToken {
            process_version: 1,
            digest: "digest".into(),
        };
        let mut handler = Handler {
            outcome: OperationOutcome::Committed(token.clone()),
            kind: OperationKind::ExecuteTask,
        };
        let result = execute_operation(&mut repository, &mut handler, request(id)).unwrap();
        assert!(
            matches!(result, OperationExecutionResult::Completed(OperationRecord { state: OperationState::Completed(ref value), .. }) if value == &token)
        );
    }
    #[test]
    fn handler_explicitly_classifies_effect_unknown() {
        let (mut repository, id) = setup(OperationKind::DeliverMessage);
        let mut handler = Handler {
            outcome: OperationOutcome::EffectUnknown,
            kind: OperationKind::DeliverMessage,
        };
        let result = execute_operation(&mut repository, &mut handler, request(id)).unwrap();
        assert!(matches!(
            result,
            OperationExecutionResult::EffectUnknown(OperationRecord {
                state: OperationState::EffectUnknown { attempt: 1 },
                ..
            })
        ));
    }
    #[test]
    fn unsupported_kind_is_rejected_before_claim_or_handler() {
        let (mut repository, id) = setup(OperationKind::FireTrigger);
        let mut handler = Handler {
            outcome: OperationOutcome::RetrySafe,
            kind: OperationKind::ExecuteTask,
        };
        assert_eq!(
            execute_operation(&mut repository, &mut handler, request(id.clone())),
            Err(OperationExecutionError::UnsupportedKind(
                OperationKind::FireTrigger
            ))
        );
        assert_eq!(
            repository.load("p").unwrap().unwrap().operations[&id].revision,
            0
        );
    }
    #[test]
    fn explicit_quarantine_is_persisted_as_terminal_state() {
        let (mut repository, id) = setup(OperationKind::RunPostCommit);
        let mut handler = Handler {
            outcome: OperationOutcome::Quarantine(
                QuarantineReason::new("invalid external contract").unwrap(),
            ),
            kind: OperationKind::RunPostCommit,
        };
        let result = execute_operation(&mut repository, &mut handler, request(id)).unwrap();
        assert!(matches!(
            result,
            OperationExecutionResult::Quarantined(OperationRecord {
                state: OperationState::Quarantined { .. },
                ..
            })
        ));
    }

    #[test]
    fn retry_attempt_is_derived_from_fence_not_handler_input() {
        let (mut repository, id) = setup(OperationKind::PropagateTask);
        let mut handler = Handler {
            outcome: OperationOutcome::RetrySafe,
            kind: OperationKind::PropagateTask,
        };
        let mut execution = request(id);
        execution.claim.fence = 7;
        let result = execute_operation(&mut repository, &mut handler, execution).unwrap();
        assert!(matches!(
            result,
            OperationExecutionResult::RetryScheduled(OperationRecord {
                state: OperationState::RetryScheduled { attempt: 7 },
                ..
            })
        ));
    }

    #[test]
    fn fence_too_large_for_persisted_attempt_is_rejected_without_wrapping() {
        let (mut repository, id) = setup(OperationKind::PropagateTask);
        let mut handler = Handler {
            outcome: OperationOutcome::RetrySafe,
            kind: OperationKind::PropagateTask,
        };
        let mut execution = request(id);
        execution.claim.fence = u64::from(u32::MAX) + 1;
        assert_eq!(
            execute_operation(&mut repository, &mut handler, execution),
            Err(OperationExecutionError::AttemptOverflow)
        );
    }
    struct RenewingHandler {
        observed_revision: Option<u64>,
        observed_heartbeat: Option<u64>,
    }

    impl OperationHandler for RenewingHandler {
        fn supports(&self, _: OperationKind) -> bool {
            true
        }

        fn execute(&mut self, context: &mut dyn OperationExecutionContext) -> OperationOutcome {
            assert!(matches!(
                context.operation().state,
                OperationState::CommitPrepared(_)
            ));
            context.renew_lease(200).unwrap();
            self.observed_revision = Some(context.operation().revision);
            self.observed_heartbeat = Some(context.claim().heartbeat_revision);
            OperationOutcome::RetrySafe
        }
    }

    #[test]
    fn renewal_updates_durable_revision_used_by_final_outcome() {
        let (mut repository, id) = setup(OperationKind::ExecuteTask);
        let mut handler = RenewingHandler {
            observed_revision: None,
            observed_heartbeat: None,
        };
        let result = execute_operation(&mut repository, &mut handler, request(id)).unwrap();
        assert_eq!(handler.observed_revision, Some(3));
        assert_eq!(handler.observed_heartbeat, Some(1));
        assert!(matches!(
            result,
            OperationExecutionResult::RetryScheduled(OperationRecord {
                revision: 4,
                state: OperationState::RetryScheduled { attempt: 1 },
                ..
            })
        ));
    }

    struct NonExtendingHandler;

    impl OperationHandler for NonExtendingHandler {
        fn supports(&self, _: OperationKind) -> bool {
            true
        }

        fn execute(&mut self, context: &mut dyn OperationExecutionContext) -> OperationOutcome {
            assert_eq!(
                context.renew_lease(context.claim().lease_until_ms),
                Err(OperationExecutionError::LeaseNotExtended)
            );
            OperationOutcome::RetrySafe
        }
    }

    #[test]
    fn renewal_must_strictly_extend_the_deadline() {
        let (mut repository, id) = setup(OperationKind::ExecuteTask);
        assert!(matches!(
            execute_operation(&mut repository, &mut NonExtendingHandler, request(id)),
            Ok(OperationExecutionResult::RetryScheduled(_))
        ));
    }

    #[test]
    fn quarantine_reason_is_validated_before_handler_output() {
        assert_eq!(QuarantineReason::new("   "), Err(InvalidQuarantineReason));
    }
}
