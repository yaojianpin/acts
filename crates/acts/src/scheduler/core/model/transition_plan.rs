use crate::scheduler::core::{
    model::{
        model_graph::NodeId,
        operation::{ClaimToken, OperationId, OperationKind},
        task_graph::{TaskId, TaskState},
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
pub(crate) struct ProcessVersion {
    /// Immutable execution-topology revision. State-only commits never change it.
    pub(crate) graph: u64,
    pub(crate) process: u64,
    pub(crate) operations: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TaskCreate {
    pub(crate) task: TaskId,
    pub(crate) model_node: NodeId,
    pub(crate) parent: Option<TaskId>,
    pub(crate) predecessors: Vec<TaskId>,
    pub(crate) initial_state: TaskState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StateChange {
    pub(crate) task: TaskId,
    pub(crate) expected_revision: u64,
    pub(crate) from: TaskState,
    pub(crate) to: TaskState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct VariableWrite {
    pub(crate) scope: TaskId,
    pub(crate) name: String,
    pub(crate) value: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OperationIntent {
    pub(crate) id: OperationId,
    pub(crate) kind: OperationKind,
    /// When this operation becomes due, in the ready index's clock. `None`
    /// means the commit instant: ordinary work is runnable as soon as it is
    /// durable. Timers and boot-admission checks name a later instant so they
    /// never enter the hot ready set.
    pub(crate) ready_at_ms: Option<i64>,
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
    pub(crate) expected: ProcessVersion,
    pub(crate) task_creates: Vec<TaskCreate>,
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
        for create in &self.task_creates {
            if !creates.insert(create.task.clone()) {
                return Err(PlanError::DuplicateTaskCreate(create.task.clone()));
            }
            if create.initial_state != TaskState::Created
                && create.initial_state != TaskState::Ready
            {
                return Err(PlanError::InvalidInitialState(
                    create.task.clone(),
                    create.initial_state,
                ));
            }
            let mut predecessors = HashSet::new();
            for predecessor in &create.predecessors {
                if predecessor == &create.task || !predecessors.insert(predecessor.clone()) {
                    return Err(PlanError::InvalidPredecessor(
                        create.task.clone(),
                        predecessor.clone(),
                    ));
                }
            }
        }
        let mut tasks = HashSet::new();
        for change in &self.state_changes {
            if !tasks.insert(change.task.clone()) {
                return Err(PlanError::DuplicateStateChange(change.task.clone()));
            }
            if change.from == change.to {
                return Err(PlanError::NoopStateChange(change.task.clone()));
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
    use TaskState as S;
    let allowed = matches!(
        (change.from, change.to),
        (S::Created, S::Ready)
            // A run parked by the driver's slot cap is seeded with its root
            // `Created` (the read model's `None` process state, the legacy
            // parked row) and admitted by driving the very operation that
            // parked it: its admission operation *is* its start operation, so a parked
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
            task: change.task.clone(),
            from: change.from,
            to: change.to,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PlanError {
    EmptyProcessId,
    DuplicateTaskCreate(TaskId),
    InvalidInitialState(TaskId, TaskState),
    InvalidPredecessor(TaskId, TaskId),
    DuplicateStateChange(TaskId),
    NoopStateChange(TaskId),
    IllegalStateChange {
        task: TaskId,
        from: TaskState,
        to: TaskState,
    },
    EmptyVariableName(TaskId),
    DuplicateVariableWrite(TaskId, String),
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
    fn task() -> TaskId {
        TaskId::new("t1", 0).unwrap()
    }
    fn plan() -> TransitionPlan {
        TransitionPlan {
            process_id: "p1".into(),
            expected: ProcessVersion {
                graph: 0,
                process: 3,
                operations: 7,
            },
            task_creates: vec![],
            state_changes: vec![StateChange {
                task: task(),
                expected_revision: 2,
                from: TaskState::Running,
                to: TaskState::Completed,
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
        p.state_changes[0].from = TaskState::Completed;
        p.state_changes[0].to = TaskState::Running;
        assert!(matches!(
            p.validate(),
            Err(PlanError::IllegalStateChange { .. })
        ));
    }

    #[test]
    fn duplicate_mutation_of_one_task_is_rejected() {
        let mut p = plan();
        p.state_changes.push(p.state_changes[0].clone());
        assert_eq!(p.validate(), Err(PlanError::DuplicateStateChange(task())));
    }

    #[test]
    fn operation_must_belong_to_process() {
        let mut p = plan();
        p.operations.push(OperationIntent {
            id: OperationId {
                process_id: "other".into(),
                task: TaskId::new("t1", 0).unwrap(),
                sequence: 1,
            },
            kind: OperationKind::PropagateTask,
            ready_at_ms: None,
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
        p.outbox[0].payload =
            "x".repeat(crate::scheduler::core::outbox::records::MAX_OUTBOX_PAYLOAD_BYTES + 1);
        assert_eq!(
            p.validate(),
            Err(PlanError::InvalidOutbox(OutboxError::PayloadTooLarge))
        );
    }
}
