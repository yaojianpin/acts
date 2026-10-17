use super::*;

/// Storage boundary required by the kernel. Implementations must make compare
/// and commit one indivisible operation. A backend unable to do that must
/// refuse the commit rather than perform check-then-write.
#[cfg(test)]
pub(crate) trait ProcessRepository {
    fn load(&self, process_id: &str) -> Result<Option<ProcessSnapshot>, CommitError>;
    fn compare_and_commit(&mut self, plan: &TransitionPlan) -> Result<CommitReceipt, CommitError>;
}

/// Durable ownership boundary. Claiming and preparing are repository
/// transitions, not process-local bookkeeping. Recovery therefore observes
/// the same state as normal execution and never guesses from a local registry.
///
/// The cursor is positional in the *recovery document* order — process key,
/// then the operation order inside that document — so a page resumes at the
/// next unread index entry instead of rescanning every recovery document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecoveryCursor {
    pub(crate) process_key: String,
    pub(crate) operation: Option<crate::scheduler::core::model::operation::OperationId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecoveryPage {
    pub(crate) operations: Vec<OperationRecord>,
    pub(crate) next: Option<RecoveryCursor>,
}

/// The positional cursor of the durable ready index: process document, then
/// the `(ready_at_ms, operation)` position inside it. Entries are sorted by
/// exactly that pair, so a page resumes at the next unread due record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReadyCursor {
    pub(crate) process_key: String,
    pub(crate) ready_at_ms: i64,
    pub(crate) operation: crate::scheduler::core::model::operation::OperationId,
}

/// One due-ready page. `observed` and `oldest_ready_at_ms` describe the index
/// entries the page walked (including future ones) so the driver can report
/// backlog and age without a second pass.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReadyPage {
    pub(crate) operations: Vec<crate::scheduler::core::model::operation::OperationId>,
    pub(crate) next: Option<ReadyCursor>,
    pub(crate) observed: usize,
    pub(crate) oldest_ready_at_ms: Option<i64>,
}

#[cfg(test)]
pub(crate) trait RecoveryRepository {
    fn recoverable_page(
        &self,
        after: Option<&RecoveryCursor>,
        limit: usize,
    ) -> Result<RecoveryPage, CommitError>;
}

#[cfg(test)]
pub(crate) trait OperationRepository {
    fn claim_operation(
        &mut self,
        id: &crate::scheduler::core::model::operation::OperationId,
        expected_revision: u64,
        claim: crate::scheduler::core::model::operation::ClaimToken,
    ) -> Result<OperationRecord, CommitError>;

    fn renew_operation(
        &mut self,
        id: &crate::scheduler::core::model::operation::OperationId,
        expected_revision: u64,
        current: crate::scheduler::core::model::operation::ClaimToken,
        renewed: crate::scheduler::core::model::operation::ClaimToken,
    ) -> Result<OperationRecord, CommitError>;

    fn prepare_operation(
        &mut self,
        id: &crate::scheduler::core::model::operation::OperationId,
        expected_revision: u64,
        claim: crate::scheduler::core::model::operation::ClaimToken,
    ) -> Result<OperationRecord, CommitError>;

    fn complete_operation(
        &mut self,
        id: &crate::scheduler::core::model::operation::OperationId,
        expected_revision: u64,
        commit: CommitToken,
    ) -> Result<OperationRecord, CommitError>;
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct MemoryProcessRepository {
    processes: HashMap<String, ProcessSnapshot>,
}

#[cfg(test)]
impl MemoryProcessRepository {
    pub(crate) fn insert(&mut self, snapshot: ProcessSnapshot) -> Result<(), CommitError> {
        snapshot.validate()?;
        if self.processes.contains_key(&snapshot.process_id) {
            return Err(CommitError::ProcessAlreadyExists(snapshot.process_id));
        }
        self.processes.insert(snapshot.process_id.clone(), snapshot);
        Ok(())
    }
}

#[cfg(test)]
impl RecoveryRepository for MemoryProcessRepository {
    fn recoverable_page(
        &self,
        after: Option<&RecoveryCursor>,
        limit: usize,
    ) -> Result<RecoveryPage, CommitError> {
        if limit == 0 {
            return Err(CommitError::InvalidPageLimit);
        }
        let mut operations = self
            .processes
            .values()
            .flat_map(|snapshot| snapshot.operations.values().cloned())
            .filter(|operation| !operation.is_retired())
            .collect::<Vec<_>>();
        operations.sort_by(|left, right| {
            left.id
                .process_id
                .cmp(&right.id.process_id)
                .then_with(|| operation_order(&left.id, &right.id))
        });
        if let Some(cursor) = after {
            operations.retain(|operation| {
                operation.id.process_id > cursor.process_key
                    || (operation.id.process_id == cursor.process_key
                        && cursor
                            .operation
                            .as_ref()
                            .is_none_or(|after| operation_order(&operation.id, after).is_gt()))
            });
        }
        let has_more = operations.len() > limit;
        operations.truncate(limit);
        let next = if has_more {
            let last = operations.last().ok_or_else(|| {
                CommitError::CorruptProcess(
                    "recovery pagination reported more rows for an empty page".into(),
                )
            })?;
            Some(RecoveryCursor {
                process_key: last.id.process_id.clone(),
                operation: Some(last.id.clone()),
            })
        } else {
            None
        };
        Ok(RecoveryPage { operations, next })
    }
}

#[cfg(test)]
pub(crate) trait MemoryProcessRepositoryAccess {
    fn insert_for_test(&mut self, snapshot: ProcessSnapshot) -> Result<(), CommitError>;
}

#[cfg(test)]
impl MemoryProcessRepositoryAccess for MemoryProcessRepository {
    fn insert_for_test(&mut self, snapshot: ProcessSnapshot) -> Result<(), CommitError> {
        self.insert(snapshot)
    }
}

#[cfg(test)]
impl OperationRepository for MemoryProcessRepository {
    fn claim_operation(
        &mut self,
        id: &crate::scheduler::core::model::operation::OperationId,
        expected_revision: u64,
        claim: crate::scheduler::core::model::operation::ClaimToken,
    ) -> Result<OperationRecord, CommitError> {
        self.evolve_operation(id, expected_revision, OperationEvent::Claim(claim))
    }

    fn renew_operation(
        &mut self,
        id: &crate::scheduler::core::model::operation::OperationId,
        expected_revision: u64,
        current: crate::scheduler::core::model::operation::ClaimToken,
        renewed: crate::scheduler::core::model::operation::ClaimToken,
    ) -> Result<OperationRecord, CommitError> {
        self.evolve_operation(
            id,
            expected_revision,
            OperationEvent::RenewClaim { current, renewed },
        )
    }

    fn prepare_operation(
        &mut self,
        id: &crate::scheduler::core::model::operation::OperationId,
        expected_revision: u64,
        claim: crate::scheduler::core::model::operation::ClaimToken,
    ) -> Result<OperationRecord, CommitError> {
        self.evolve_operation(
            id,
            expected_revision,
            OperationEvent::PrepareCommit { claim },
        )
    }

    fn complete_operation(
        &mut self,
        id: &crate::scheduler::core::model::operation::OperationId,
        expected_revision: u64,
        commit: CommitToken,
    ) -> Result<OperationRecord, CommitError> {
        self.evolve_operation(id, expected_revision, OperationEvent::Complete { commit })
    }
}

/// Advance the operation lifecycle in place.
///
/// The durable repository owns the loaded snapshot, never reuses it after a
/// failed commit (it drops the aggregate and reports the failure, and recovery
/// re-derives the operation), so it can hand the snapshot over instead of paying for
/// a clone of the whole aggregate per commit. Every check runs before the
/// first mutation.
#[cfg(test)]
impl MemoryProcessRepository {
    fn evolve_operation(
        &mut self,
        id: &crate::scheduler::core::model::operation::OperationId,
        expected_revision: u64,
        event: OperationEvent,
    ) -> Result<OperationRecord, CommitError> {
        let current = self
            .processes
            .get(&id.process_id)
            .ok_or_else(|| CommitError::ProcessNotFound(id.process_id.clone()))?;
        let mut next = current.clone();
        let operation = advance_operation_event(&mut next, id, expected_revision, event)?;
        self.processes.insert(id.process_id.clone(), next);
        Ok(operation)
    }
}

#[cfg(test)]
impl ProcessRepository for MemoryProcessRepository {
    fn load(&self, process_id: &str) -> Result<Option<ProcessSnapshot>, CommitError> {
        let snapshot = self.processes.get(process_id).cloned();
        if let Some(value) = &snapshot {
            value.validate()?;
        }
        Ok(snapshot)
    }

    fn compare_and_commit(&mut self, plan: &TransitionPlan) -> Result<CommitReceipt, CommitError> {
        let current = self
            .processes
            .get(&plan.process_id)
            .ok_or_else(|| CommitError::ProcessNotFound(plan.process_id.clone()))?;
        let mut next = current.clone();
        let receipt = advance_transition(&mut next, plan)?;
        self.processes.insert(plan.process_id.clone(), next);
        Ok(receipt)
    }
}
