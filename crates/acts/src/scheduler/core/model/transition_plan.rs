use crate::scheduler::core::{
    model::{
        execution_graph::{ExecutionId, ExecutionState},
        model_graph::NodeId,
        operation::{ClaimToken, OperationId, OperationKind},
    },
    outbox::records::{OutboxError, OutboxId, OutboxRecord},
};

use std::collections::HashSet;
use std::fmt;

/// The counters a commit advances. They only ever grow, and any two
/// *committed* versions are totally ordered (a commit wins one CAS and the
/// loser re-plans from the winner's version), which is what lets a derived
/// read model drop a projection that is older than the rows it would write
/// (see `RowProjector::claim`).
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
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
    /// The receipt the hop's own transport returned for this record, when it
    /// delivered it *before* committing (the effect seam's ordering: the work
    /// runs, then one commit records it). The commit creates the record
    /// already `Delivered` — no claim, no confirm pass, no second commit — and
    /// `None` leaves it `Pending` for the driver's retry pass (a transport that
    /// refused, or a plan no hop has offered the transport yet).
    pub(crate) receipt: Option<String>,
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
}

fn validate_state_change(change: &StateChange) -> Result<(), PlanError> {
    use ExecutionState as S;
    let allowed = matches!(
        (change.from, change.to),
        (S::Created, S::Ready)
            // A run parked by the driver's slot cap is seeded with its root
            // `Created` (the read model's `None` process state, the legacy
            // parked row) and admitted by driving the very operation that
            // parked it: its admission hop *is* its start hop, so a parked
            // root takes its act straight to `Running`/`Waiting` — the legacy
            // parked process went from `None` to its root task running with no
            // scheduling step in between.
            | (S::Created, S::Running | S::Waiting)
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
            receipt: None,
        });
        assert_eq!(
            p.validate(),
            Err(PlanError::InvalidOutbox(OutboxError::InvalidTopic))
        );

        p.outbox[0].topic = "topic".into();
        p.outbox[0].payload =
            "x".repeat(crate::scheduler::core::outbox::records::MAX_OUTBOX_PAYLOAD_BYTES + 1);
        assert_eq!(
            p.validate(),
            Err(PlanError::InvalidOutbox(OutboxError::PayloadTooLarge))
        );
    }
}
