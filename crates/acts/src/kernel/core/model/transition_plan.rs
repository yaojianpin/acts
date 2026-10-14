use super::outbox::{OutboxError, OutboxId, OutboxRecord};
use super::{ClaimToken, ExecutionId, ExecutionState, NodeId, OperationId, OperationKind};
use std::collections::{BTreeMap, HashSet};
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AggregateVersion {
    /// Immutable execution-topology revision. State-only commits never change it.
    pub(crate) graph: u64,
    pub(crate) process: u64,
    pub(crate) operations: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExecutionCreate {
    pub(crate) execution: ExecutionId,
    pub(crate) model_node: NodeId,
    pub(crate) parent: Option<ExecutionId>,
    pub(crate) predecessors: Vec<ExecutionId>,
    pub(crate) initial_state: ExecutionState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StateChange {
    pub(crate) execution: ExecutionId,
    pub(crate) expected_revision: u64,
    pub(crate) from: ExecutionState,
    pub(crate) to: ExecutionState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct VariableWrite {
    pub(crate) scope: ExecutionId,
    pub(crate) name: String,
    pub(crate) value: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OperationIntent {
    pub(crate) id: OperationId,
    pub(crate) kind: OperationKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OperationCommit {
    pub(crate) id: OperationId,
    pub(crate) claim: ClaimToken,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OutboxIntent {
    pub(crate) key: String,
    pub(crate) topic: String,
    pub(crate) payload: String,
}

/// Complete, value-only output of a planner. It contains no live process,
/// cache, queue, store or channel handles and is therefore deterministic and
/// safe to retry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TransitionPlan {
    pub(crate) process_id: String,
    pub(crate) expected: AggregateVersion,
    pub(crate) execution_creates: Vec<ExecutionCreate>,
    pub(crate) state_changes: Vec<StateChange>,
    pub(crate) variable_writes: Vec<VariableWrite>,
    pub(crate) source_operation: Option<OperationCommit>,
    pub(crate) operations: Vec<OperationIntent>,
    pub(crate) outbox: Vec<OutboxIntent>,
}

impl TransitionPlan {
    pub(crate) fn validate(&self) -> Result<(), PlanError> {
        if self.process_id.trim().is_empty() {
            return Err(PlanError::EmptyProcessId);
        }
        let mut creates = HashSet::new();
        for create in &self.execution_creates {
            if !creates.insert(create.execution.clone()) {
                return Err(PlanError::DuplicateExecutionCreate(
                    create.execution.clone(),
                ));
            }
            if create.initial_state != ExecutionState::Created
                && create.initial_state != ExecutionState::Ready
            {
                return Err(PlanError::InvalidInitialState(
                    create.execution.clone(),
                    create.initial_state,
                ));
            }
            let mut predecessors = HashSet::new();
            for predecessor in &create.predecessors {
                if predecessor == &create.execution || !predecessors.insert(predecessor.clone()) {
                    return Err(PlanError::InvalidPredecessor(
                        create.execution.clone(),
                        predecessor.clone(),
                    ));
                }
            }
        }
        let mut executions = HashSet::new();
        for change in &self.state_changes {
            if !executions.insert(change.execution.clone()) {
                return Err(PlanError::DuplicateStateChange(change.execution.clone()));
            }
            if change.from == change.to {
                return Err(PlanError::NoopStateChange(change.execution.clone()));
            }
            validate_state_change(change)?;
        }
        let mut vars = HashSet::new();
        for write in &self.variable_writes {
            if write.name.trim().is_empty() {
                return Err(PlanError::EmptyVariableName(write.scope.clone()));
            }
            if !vars.insert((write.scope.clone(), write.name.clone())) {
                return Err(PlanError::DuplicateVariableWrite(
                    write.scope.clone(),
                    write.name.clone(),
                ));
            }
        }
        if let Some(source) = &self.source_operation
            && source.id.process_id != self.process_id
        {
            return Err(PlanError::ForeignOperation(source.id.clone()));
        }
        let mut operations = HashSet::new();
        for op in &self.operations {
            if op.id.process_id != self.process_id {
                return Err(PlanError::ForeignOperation(op.id.clone()));
            }
            if !operations.insert(op.id.clone()) {
                return Err(PlanError::DuplicateOperation(op.id.clone()));
            }
        }
        let mut outbox = HashSet::new();
        for item in &self.outbox {
            let id = OutboxId {
                process_id: self.process_id.clone(),
                key: item.key.clone(),
            };
            OutboxRecord::pending(id, item.topic.clone(), item.payload.clone(), 0)
                .map_err(PlanError::InvalidOutbox)?;
            if !outbox.insert(item.key.clone()) {
                return Err(PlanError::DuplicateOutboxKey(item.key.clone()));
            }
        }
        Ok(())
    }

    /// Canonical variable projection used by committers. Ordering is stable,
    /// independent of hash-map iteration and therefore suitable for digests.
    pub(crate) fn canonical_variables(&self) -> BTreeMap<(ExecutionId, String), String> {
        self.variable_writes
            .iter()
            .map(|w| ((w.scope.clone(), w.name.clone()), w.value.clone()))
            .collect()
    }

    /// Stable outbox projection. Message bodies remain immutable and ordering
    /// never depends on hash iteration, so repositories and digests can share it.
    pub(crate) fn canonical_outbox(&self) -> Vec<&OutboxIntent> {
        let mut items = self.outbox.iter().collect::<Vec<_>>();
        items.sort_by(|left, right| left.key.cmp(&right.key));
        items
    }
}

fn validate_state_change(change: &StateChange) -> Result<(), PlanError> {
    use ExecutionState as S;
    let allowed = matches!(
        (change.from, change.to),
        (S::Created, S::Ready)
            | (S::Ready, S::Running)
            // A waiting package parks the act right after it was scheduled:
            // the act ran into an external wait (an `Interrupt` in the legacy
            // task vocabulary) without ever being `Running`.
            | (S::Ready, S::Waiting)
            | (S::Running, S::Waiting)
            | (S::Waiting, S::Running)
            | (
                S::Running,
                S::Completed | S::Submitted | S::Failed | S::Aborted | S::Skipped
            )
            // A `Cancel` *is* the legacy rewind: the legacy handler refuses
            // unless the act's step already finished its own work, then closes
            // that act `Backed` and redoes the step (see `ActionPlanner`). So
            // the decided generation of a finished step moves to `Backed`,
            // exactly like a `Back`'s source — never from `Ready`/`Running`/
            // `Waiting`, which the legacy guard refuses.
            | (S::Completed | S::Submitted, S::Backed)
            | (
                S::Waiting,
                S::Completed
                    | S::Submitted
                    | S::Backed
                    | S::Failed
                    | S::Aborted
                    | S::Skipped
                    | S::Removed
            )
            | (S::Ready, S::Skipped | S::Removed)
            // A branch predicate is evaluated in the branch task's `init` (the
            // legacy `Branch::init`), before the task ever runs: one that
            // cannot be evaluated decides that branch `Error` straight from
            // `Ready`.
            | (S::Ready, S::Failed)
            // The abort cascade's upward walk decides EVERY ancestor of an
            // aborted execution `Aborted` — including already-terminal ones —
            // exactly like the legacy ancestor walk that terminates the run
            // from the acting act up through its ancestors. The error walk
            // does the same with `Failed` (`emit_error` → `parent.set_err`).
            | (
                S::Completed | S::Submitted | S::Backed | S::Cancelled | S::Failed | S::Skipped | S::Removed,
                S::Aborted | S::Failed,
            )
    );
    if allowed {
        Ok(())
    } else {
        Err(PlanError::IllegalStateChange {
            execution: change.execution.clone(),
            from: change.from,
            to: change.to,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PlanError {
    EmptyProcessId,
    DuplicateExecutionCreate(ExecutionId),
    InvalidInitialState(ExecutionId, ExecutionState),
    InvalidPredecessor(ExecutionId, ExecutionId),
    DuplicateStateChange(ExecutionId),
    NoopStateChange(ExecutionId),
    IllegalStateChange {
        execution: ExecutionId,
        from: ExecutionState,
        to: ExecutionState,
    },
    EmptyVariableName(ExecutionId),
    DuplicateVariableWrite(ExecutionId, String),
    ForeignOperation(OperationId),
    DuplicateOperation(OperationId),
    InvalidOutbox(OutboxError),
    DuplicateOutboxKey(String),
}
impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "transition plan error: {self:?}")
    }
}
impl std::error::Error for PlanError {}

#[cfg(test)]
mod tests {
    use super::*;
    fn execution() -> ExecutionId {
        ExecutionId::new("t1", 0).unwrap()
    }
    fn plan() -> TransitionPlan {
        TransitionPlan {
            process_id: "p1".into(),
            expected: AggregateVersion {
                graph: 0,
                process: 3,
                operations: 7,
            },
            execution_creates: vec![],
            state_changes: vec![StateChange {
                execution: execution(),
                expected_revision: 2,
                from: ExecutionState::Running,
                to: ExecutionState::Completed,
            }],
            variable_writes: vec![],
            source_operation: None,
            operations: vec![],
            outbox: vec![],
        }
    }

    #[test]
    fn terminal_state_cannot_be_reopened_arbitrarily() {
        let mut p = plan();
        p.state_changes[0].from = ExecutionState::Completed;
        p.state_changes[0].to = ExecutionState::Running;
        assert!(matches!(
            p.validate(),
            Err(PlanError::IllegalStateChange { .. })
        ));
    }

    #[test]
    fn duplicate_mutation_of_one_execution_is_rejected() {
        let mut p = plan();
        p.state_changes.push(p.state_changes[0].clone());
        assert_eq!(
            p.validate(),
            Err(PlanError::DuplicateStateChange(execution()))
        );
    }

    #[test]
    fn operation_must_belong_to_aggregate() {
        let mut p = plan();
        p.operations.push(OperationIntent {
            id: OperationId {
                process_id: "other".into(),
                task_id: "t1".into(),
                generation: 0,
                sequence: 1,
            },
            kind: OperationKind::PropagateTask,
        });
        assert!(matches!(p.validate(), Err(PlanError::ForeignOperation(_))));
    }
    #[test]
    fn rejects_outbox_messages_that_cannot_be_persisted() {
        let mut p = plan();
        p.outbox.push(OutboxIntent {
            key: "k".into(),
            topic: " ".into(),
            payload: "{}".into(),
        });
        assert_eq!(
            p.validate(),
            Err(PlanError::InvalidOutbox(OutboxError::InvalidTopic))
        );

        p.outbox[0].topic = "topic".into();
        p.outbox[0].payload = "x".repeat(crate::kernel::outbox::MAX_OUTBOX_PAYLOAD_BYTES + 1);
        assert_eq!(
            p.validate(),
            Err(PlanError::InvalidOutbox(OutboxError::PayloadTooLarge))
        );
    }

    #[test]
    fn canonical_outbox_is_key_ordered_without_mutating_the_plan() {
        let mut p = plan();
        p.outbox = vec![
            OutboxIntent {
                key: "z".into(),
                topic: "topic.z".into(),
                payload: "z".into(),
            },
            OutboxIntent {
                key: "a".into(),
                topic: "topic.a".into(),
                payload: "a".into(),
            },
        ];
        let keys = p
            .canonical_outbox()
            .into_iter()
            .map(|item| item.key.as_str())
            .collect::<Vec<_>>();
        assert_eq!(keys, vec!["a", "z"]);
        assert_eq!(p.outbox[0].key, "z");
    }
}
