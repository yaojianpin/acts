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

impl ProcessVersion {
    /// The durable read-model token: `"<graph>.<process>.<operations>"`.
    ///
    /// Every projected row stores this string as its `source_revision`; the
    /// store's conditional write parses it back and applies the row only when
    /// the incoming version is strictly newer. The fields are decimal and the
    /// derived `Ord` is lexicographic, so the string form uses the same order
    /// (the components are compared as numbers after parsing, never as text).
    pub(crate) fn encode(&self) -> String {
        format!("{}.{}.{}", self.graph, self.process, self.operations)
    }

    /// Parse a `source_revision` token. `None` for a missing/invalid/legacy
    /// token (rows written before A5 carry no revision): such a row is older
    /// than any committed aggregate version.
    pub(crate) fn decode(raw: &str) -> Option<Self> {
        let mut parts = raw.split('.');
        let graph = parts.next()?.parse().ok()?;
        let process = parts.next()?.parse().ok()?;
        let operations = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            graph,
            process,
            operations,
        })
    }
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
                // A pure-internal cascade folded into the creator's own plan
                // decides an execution the same batch creates (an unselected
                // branch entering the skip cascade, an aborted running
                // successor). The aggregate never observes the intermediate
                // state the separate operation would have written, so the
                // execution is born in the state the cascade decided.
                && !matches!(
                    create.initial_state,
                    TaskState::Skipped | TaskState::Aborted
                )
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

    #[test]
    fn the_durable_version_token_round_trips_and_orders() {
        let low = ProcessVersion {
            graph: 1,
            process: 2,
            operations: 3,
        };
        let high = ProcessVersion {
            graph: 1,
            process: 2,
            operations: 4,
        };
        assert_eq!(ProcessVersion::decode(&low.encode()), Some(low.clone()));
        assert!(low < high);
        assert_eq!(ProcessVersion::decode(""), None);
        assert_eq!(ProcessVersion::decode("1.2"), None);
        assert_eq!(ProcessVersion::decode("1.2.3.4"), None);
        assert_eq!(ProcessVersion::decode("1.x.3"), None);
    }

    fn create(task: &str, generation: u64, state: TaskState) -> TaskCreate {
        TaskCreate {
            task: TaskId::new(task, generation).unwrap(),
            model_node: crate::scheduler::core::model::model_graph::NodeId::parse(task).unwrap(),
            parent: None,
            predecessors: vec![],
            initial_state: state,
        }
    }

    #[test]
    fn an_empty_process_id_is_refused() {
        let mut p = plan();
        p.process_id = " ".into();
        assert_eq!(p.validate(), Err(PlanError::EmptyProcessId));
    }

    #[test]
    fn task_creates_are_unique_and_sane() {
        let mut p = plan();
        p.state_changes.clear();
        p.task_creates = vec![
            create("a", 0, TaskState::Ready),
            create("a", 0, TaskState::Created),
        ];
        assert_eq!(
            p.validate(),
            Err(PlanError::DuplicateTaskCreate(TaskId::new("a", 0).unwrap()))
        );

        let mut p = plan();
        p.state_changes.clear();
        p.task_creates = vec![create("a", 0, TaskState::Running)];
        assert_eq!(
            p.validate(),
            Err(PlanError::InvalidInitialState(
                TaskId::new("a", 0).unwrap(),
                TaskState::Running
            ))
        );

        // A cascade may fold its decision into the create.
        for state in [TaskState::Skipped, TaskState::Aborted] {
            let mut p = plan();
            p.state_changes.clear();
            p.task_creates = vec![create("a", 0, state)];
            assert_eq!(p.validate(), Ok(()), "{state:?} is a legal birth state");
        }
    }

    #[test]
    fn predecessors_must_be_other_tasks_and_unique() {
        for predecessors in [
            vec![TaskId::new("a", 0).unwrap()],
            vec![TaskId::new("b", 0).unwrap(), TaskId::new("b", 0).unwrap()],
        ] {
            let mut p = plan();
            p.state_changes.clear();
            let mut created = create("a", 0, TaskState::Created);
            created.predecessors = predecessors;
            p.task_creates = vec![created];
            assert!(matches!(
                p.validate(),
                Err(PlanError::InvalidPredecessor(_, _))
            ));
        }
    }

    #[test]
    fn variable_writes_are_named_and_unique() {
        let mut p = plan();
        p.state_changes.clear();
        p.variable_writes = vec![VariableWrite {
            scope: TaskId::new("t1", 0).unwrap(),
            name: "  ".into(),
            value: "v".into(),
        }];
        assert_eq!(p.validate(), Err(PlanError::EmptyVariableName(task())));

        let mut p = plan();
        p.state_changes.clear();
        p.variable_writes = vec![
            VariableWrite {
                scope: task(),
                name: "auto".into(),
                value: "1".into(),
            },
            VariableWrite {
                scope: task(),
                name: "auto".into(),
                value: "0".into(),
            },
        ];
        assert_eq!(
            p.validate(),
            Err(PlanError::DuplicateVariableWrite(task(), "auto".into()))
        );
    }

    #[test]
    fn the_source_operation_must_belong_to_the_plan_process() {
        let mut p = plan();
        p.source_operation = Some(
            crate::scheduler::core::model::transition_plan::OperationCommit {
                id: OperationId {
                    process_id: "other".into(),
                    task: task(),
                    sequence: 3,
                },
                claim: crate::scheduler::core::model::operation::ClaimToken {
                    owner: "r".into(),
                    fence: 1,
                    lease_until_ms: 10,
                    heartbeat_revision: 0,
                },
            },
        );
        assert!(matches!(p.validate(), Err(PlanError::ForeignOperation(_))));
    }

    #[test]
    fn operations_are_unique() {
        let mut p = plan();
        let intent = OperationIntent {
            id: OperationId {
                process_id: "p1".into(),
                task: task(),
                sequence: 1,
            },
            kind: OperationKind::PropagateTask,
            ready_at_ms: None,
        };
        p.operations = vec![intent.clone(), intent.clone()];
        assert_eq!(
            p.validate(),
            Err(PlanError::DuplicateOperation(intent.id.clone()))
        );
    }

    #[test]
    fn outbox_keys_are_unique() {
        let mut p = plan();
        let item = OutboxIntent {
            key: "k".into(),
            topic: "topic".into(),
            payload: "{}".into(),
        };
        p.outbox = vec![item.clone(), item];
        assert_eq!(p.validate(), Err(PlanError::DuplicateOutboxKey("k".into())));
    }

    #[test]
    fn plan_errors_render() {
        assert!(
            PlanError::EmptyProcessId
                .to_string()
                .starts_with("transition plan error:")
        );
    }
}
