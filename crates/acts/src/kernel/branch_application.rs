use super::application::{OperationWakeSink, WakeError};
use super::branch::{BranchCommand, BranchPlan, BranchPlanner};
use super::commit::{AggregateRepository, CommitError, CommitReceipt, OperationRepository};
use super::{ClaimToken, OperationId};
use std::fmt;

/// Durable orchestration for branch evaluation. Deferral is persisted as a
/// retryable operation state, never as quarantine and never as business-state
/// mutation. Ready plans use the same atomic commit boundary as actions.
pub(crate) struct BranchApplicationService<R, W> {
    repository: R,
    wake_sink: W,
}

impl<R, W> BranchApplicationService<R, W>
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
        mut command: BranchCommand,
        expected_operation_revision: u64,
        claim: ClaimToken,
    ) -> Result<BranchExecution, BranchApplicationError> {
        BranchPlanner::validate_command(&command)
            .map_err(|error| BranchApplicationError::Planning(error.to_string()))?;
        if command.source_operation.claim != claim {
            return Err(BranchApplicationError::ClaimEnvelopeMismatch);
        }
        let operation_id = command.source_operation.id.clone();
        let snapshot = self
            .repository
            .load(&command.process_id)?
            .ok_or_else(|| BranchApplicationError::AggregateNotFound(command.process_id.clone()))?;
        let preflight = BranchPlanner::plan(&snapshot, &command)
            .map_err(|error| BranchApplicationError::Planning(error.to_string()))?;

        let claimed = self.repository.claim_operation(
            &operation_id,
            expected_operation_revision,
            claim.clone(),
        )?;

        if let BranchPlan::Deferred { dependencies } = preflight {
            let attempt =
                u32::try_from(claim.fence).map_err(|_| BranchApplicationError::AttemptOverflow)?;
            let retry = self.repository.schedule_operation_retry(
                &operation_id,
                claimed.revision,
                claim,
                attempt,
            )?;
            return Ok(BranchExecution::Deferred {
                dependencies,
                retry_revision: retry.revision,
            });
        }

        let prepared =
            self.repository
                .prepare_operation(&operation_id, claimed.revision, claim.clone())?;
        let authoritative = self
            .repository
            .load(&command.process_id)?
            .ok_or_else(|| BranchApplicationError::AggregateNotFound(command.process_id.clone()))?;
        command.source_operation.claim = claim.clone();
        let plan = match BranchPlanner::plan(&authoritative, &command) {
            Ok(BranchPlan::Ready(plan)) => plan,
            Ok(BranchPlan::Deferred { dependencies }) => {
                let attempt = u32::try_from(claim.fence)
                    .map_err(|_| BranchApplicationError::AttemptOverflow)?;
                let retry = self.repository.schedule_operation_retry(
                    &operation_id,
                    prepared.revision,
                    claim,
                    attempt,
                )?;
                return Ok(BranchExecution::Deferred {
                    dependencies,
                    retry_revision: retry.revision,
                });
            }
            Err(error) => {
                let attempt = u32::try_from(claim.fence)
                    .map_err(|_| BranchApplicationError::AttemptOverflow)?;
                self.repository.schedule_operation_retry(
                    &operation_id,
                    prepared.revision,
                    claim,
                    attempt,
                )?;
                return Err(BranchApplicationError::AuthoritativeStateChanged(
                    error.to_string(),
                ));
            }
        };

        let receipt = self.repository.compare_and_commit(&plan)?;
        let committed = self
            .repository
            .load(&command.process_id)?
            .and_then(|snapshot| snapshot.operations.get(&operation_id).cloned())
            .ok_or_else(|| BranchApplicationError::OperationLost(operation_id.clone()))?;
        let completed = self.repository.complete_operation(
            &operation_id,
            committed.revision,
            receipt.token.clone(),
        )?;

        let mut wake_failures = Vec::new();
        for operation in &receipt.new_operations {
            if let Err(error) = self.wake_sink.wake(operation) {
                wake_failures.push((operation.clone(), error));
            }
        }
        Ok(BranchExecution::Committed {
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
pub(crate) enum BranchExecution {
    Deferred {
        dependencies: Vec<String>,
        retry_revision: u64,
    },
    Committed {
        receipt: CommitReceipt,
        completed_revision: u64,
        wake_failures: Vec<(OperationId, WakeError)>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum BranchApplicationError {
    AggregateNotFound(String),
    OperationLost(OperationId),
    ClaimEnvelopeMismatch,
    Planning(String),
    AuthoritativeStateChanged(String),
    AttemptOverflow,
    Repository(CommitError),
}

impl From<CommitError> for BranchApplicationError {
    fn from(error: CommitError) -> Self {
        Self::Repository(error)
    }
}

impl fmt::Display for BranchApplicationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "branch application error: {self:?}")
    }
}

impl std::error::Error for BranchApplicationError {}
