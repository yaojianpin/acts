//! Container fan-out and join planning.
//!
//! A *container* execution holds model children: a step with `branches`, or a
//! branch with its own `steps`. Instead of completing at its effect point, a
//! container fans out — every child becomes an execution in one atomic plan —
//! and parks in `Waiting` until all of its children are terminal (the join).
//! This mirrors the legacy engine, where `check_in_children` schedules the
//! children of a running step and `auto_complete` finishes the step once they
//! all settled.
//!
//! The planner is pure: it receives the snapshot, the already-evaluated
//! selection (branch predicates are the host's job, selection semantics are
//! [`super::BranchPlanner`]'s), and the per-child durable facts (`auto`,
//! declared vars). Admission of each child goes through
//! [`super::GenerationAdmissionPolicy`], so a re-entered container reuses the
//! generations a Cancel rewind already recreated instead of allocating
//! duplicates.

use super::{
    AggregateSnapshot, ExecutionCreate, ExecutionGraph, ExecutionId, ExecutionState,
    ExistingGeneration, GenerationAdmissionPolicy, NodeId, OperationCommit, OperationId,
    OperationIntent, OperationKind, OutboxIntent, SlotObservation, StateChange, TransitionPlan,
    VariableWrite,
};
use std::collections::HashMap;
use std::fmt;

/// What a fan-out child is admitted as.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ContainerChildPlan {
    /// Runs now: created `Ready` (or readmitted from `Created`) with an
    /// `ExecuteTask` operation.
    Entry,
    /// Not selected: created `Ready` with a `SkipTask` operation, so the
    /// parity floor keeps one terminal execution per model node (the legacy
    /// engine keeps the skipped branch task).
    Skip,
    /// A chain successor: created `Created`; ordinary propagation admits it
    /// when its predecessor settles.
    Queued,
}

/// One child execution to admit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ContainerChild {
    pub(crate) task_id: String,
    pub(crate) model_node: NodeId,
    pub(crate) plan: ContainerChildPlan,
    /// Edges of the created execution. `Source` is the fanning-out
    /// container; `Sibling(i)` the i-th child of the same command, so the
    /// host can chain children whose generations only the planner computes.
    pub(crate) predecessors: Vec<ContainerPredecessor>,
    /// Declared model vars, seeded as this execution's own scope (JSON
    /// strings, the aggregate's variable representation).
    pub(crate) vars: Vec<(String, String)>,
    pub(crate) auto: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ContainerPredecessor {
    Source,
    Sibling(usize),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ContainerFanoutCommand {
    pub(crate) process_id: String,
    pub(crate) source: ExecutionId,
    pub(crate) source_operation: OperationCommit,
    pub(crate) children: Vec<ContainerChild>,
    /// Client inputs of the completion that fans out (an auto completion has
    /// none). They land on the container's own scope, exactly like the
    /// action-variable writes of a plain completion.
    pub(crate) inputs: Vec<(String, String)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ContainerJoinCommand {
    pub(crate) process_id: String,
    pub(crate) parent: ExecutionId,
}

pub(crate) struct ContainerPlanner;

impl ContainerPlanner {
    /// Fan the container's children out in one atomic plan. The source parks
    /// in `Waiting`; the join (see [`Self::plan_join`]) completes it once
    /// every child is terminal. An empty container completes immediately.
    pub(crate) fn plan_fanout(
        snapshot: &AggregateSnapshot,
        command: &ContainerFanoutCommand,
    ) -> Result<TransitionPlan, ContainerError> {
        if snapshot.process_id != command.process_id {
            return Err(ContainerError::WrongAggregate);
        }
        let source = snapshot
            .executions
            .get(&command.source)
            .ok_or_else(|| ContainerError::ExecutionNotFound(command.source.clone()))?;
        if !matches!(
            source.state,
            ExecutionState::Running | ExecutionState::Waiting
        ) {
            return Err(ContainerError::SourceNotContainer {
                source: command.source.clone(),
                state: source.state,
            });
        }
        let operation = &command.source_operation.id;
        if operation.process_id != command.process_id
            || operation.task_id != command.source.task_id
            || operation.generation != command.source.generation
        {
            return Err(ContainerError::SourceOperationTargetsDifferentExecution);
        }
        if command.children.is_empty() {
            // A container without children is a normal completion: the join
            // of zero children fires in the same hop.
            return Self::empty_container_plan(snapshot, command, source.state, source.revision);
        }

        let mut creates = Vec::new();
        // The source parks in `Waiting` — unless a client completion found it
        // already waiting, in which case the state is already the parked one.
        let mut state_changes = Vec::new();
        if source.state != ExecutionState::Waiting {
            state_changes.push(StateChange {
                execution: command.source.clone(),
                expected_revision: source.revision,
                from: source.state,
                to: ExecutionState::Waiting,
            });
        }
        let mut variable_writes: Vec<VariableWrite> = command
            .inputs
            .iter()
            .map(|(name, value)| VariableWrite {
                scope: command.source.clone(),
                name: name.clone(),
                value: value.clone(),
            })
            .collect();
        let mut operations = Vec::new();
        // Resolved execution of each already-processed child, for sibling
        // predecessor references.
        let mut resolved: Vec<ExecutionId> = Vec::with_capacity(command.children.len());
        for (index, child) in command.children.iter().enumerate() {
            if child.task_id.trim().is_empty() {
                return Err(ContainerError::EmptyChildTaskId);
            }
            let observation = SlotObservation::vacant(false, false)
                .with_existing(existing_generation(snapshot, &child.task_id));
            let generation = next_generation(snapshot, &child.task_id)
                .ok_or(ContainerError::GenerationOverflow)?;
            let execution = match super::GenerationAdmissionPolicy::decide(
                super::ExecutionSlotKind::Once,
                super::AdmissionIntent::Forward,
                observation,
            ) {
                super::AdmissionDecision::Ignore => {
                    resolved.push(
                        latest_execution(snapshot, &child.task_id)
                            .ok_or(ContainerError::ExecutionNotFound(command.source.clone()))?,
                    );
                    continue;
                }
                super::AdmissionDecision::AllocateGeneration => {
                    let execution = ExecutionId::new(child.task_id.clone(), generation)
                        .map_err(|_| ContainerError::EmptyChildTaskId)?;
                    creates.push(ExecutionCreate {
                        execution: execution.clone(),
                        model_node: child.model_node.clone(),
                        parent: Some(command.source.clone()),
                        predecessors: resolve_predecessors(
                            &child.predecessors,
                            &command.source,
                            &resolved,
                            index,
                        )?,
                        initial_state: match child.plan {
                            ContainerChildPlan::Queued => ExecutionState::Created,
                            ContainerChildPlan::Entry | ContainerChildPlan::Skip => {
                                ExecutionState::Ready
                            }
                        },
                    });
                    if let Some(intent) = child_operation(command, &execution, child.plan)? {
                        operations.push(intent);
                    }
                    child_variables(&mut variable_writes, &execution, child);
                    execution
                }
                super::AdmissionDecision::ReadmitExisting => {
                    // The latest generation exists but never ran (a Cancel
                    // rewind recreated it): admit it in place instead of
                    // allocating a second live generation.
                    let execution = latest_execution(snapshot, &child.task_id)
                        .ok_or(ContainerError::ExecutionNotFound(command.source.clone()))?;
                    if let Some(change) = readmit_change(snapshot, &execution)? {
                        state_changes.push(change);
                    }
                    if let Some(intent) = child_operation(command, &execution, child.plan)? {
                        operations.push(intent);
                    }
                    child_variables(&mut variable_writes, &execution, child);
                    execution
                }
            };
            resolved.push(execution);
        }

        let plan = TransitionPlan {
            process_id: command.process_id.clone(),
            expected: snapshot.version.clone(),
            execution_creates: creates,
            state_changes,
            variable_writes,
            source_operation: Some(command.source_operation.clone()),
            operations,
            outbox: vec![fanout_outbox(command, "selected")],
        };
        plan.validate()
            .map_err(|error| ContainerError::InvalidPlan(error.to_string()))?;
        Ok(plan)
    }

    fn empty_container_plan(
        snapshot: &AggregateSnapshot,
        command: &ContainerFanoutCommand,
        state: ExecutionState,
        revision: u64,
    ) -> Result<TransitionPlan, ContainerError> {
        let sequence = command
            .source_operation
            .id
            .sequence
            .checked_add(1)
            .ok_or(ContainerError::SequenceOverflow)?;
        let mut operations = Vec::new();
        let propagate_id = OperationId {
            process_id: command.process_id.clone(),
            task_id: command.source.task_id.clone(),
            generation: command.source.generation,
            sequence,
        };
        if !snapshot.operations.contains_key(&propagate_id) {
            operations.push(OperationIntent {
                id: propagate_id,
                kind: OperationKind::PropagateTask,
            });
        }
        let plan = TransitionPlan {
            process_id: command.process_id.clone(),
            expected: snapshot.version.clone(),
            execution_creates: Vec::new(),
            state_changes: vec![StateChange {
                execution: command.source.clone(),
                expected_revision: revision,
                from: state,
                to: ExecutionState::Completed,
            }],
            variable_writes: Vec::new(),
            source_operation: Some(command.source_operation.clone()),
            operations,
            outbox: vec![fanout_outbox(command, "empty")],
        };
        plan.validate()
            .map_err(|error| ContainerError::InvalidPlan(error.to_string()))?;
        Ok(plan)
    }

    /// The join: when a container's children are all terminal (and none was
    /// aborted — an abort owns the container's outcome, exactly like the
    /// legacy `auto_complete`), the `Waiting` container completes and hands
    /// propagation to its own chain successor. `None` means "not yet".
    pub(crate) fn plan_join(
        graph: &ExecutionGraph,
        snapshot: &AggregateSnapshot,
        command: &ContainerJoinCommand,
    ) -> Result<Option<(StateChange, OperationIntent)>, ContainerError> {
        if snapshot.process_id != command.process_id || graph.process_id != command.process_id {
            return Err(ContainerError::WrongAggregate);
        }
        if graph.revision != snapshot.version.graph {
            return Err(ContainerError::GraphVersionConflict {
                graph: graph.revision,
                snapshot: snapshot.version.graph,
            });
        }
        let parent = snapshot
            .executions
            .get(&command.parent)
            .ok_or_else(|| ContainerError::ExecutionNotFound(command.parent.clone()))?;
        if parent.state != ExecutionState::Waiting {
            return Ok(None);
        }
        let children = graph.children(&command.parent);
        if children.is_empty() {
            return Ok(None);
        }
        for child in children {
            let state = snapshot
                .executions
                .get(child)
                .ok_or_else(|| ContainerError::ExecutionNotFound(child.clone()))?
                .state;
            if !state.is_terminal() {
                return Ok(None);
            }
            if state == ExecutionState::Aborted {
                // The abort walk owns this container's outcome; completing it
                // here would double-decide (legacy `auto_complete` refuses).
                return Ok(None);
            }
        }
        let sequence = snapshot
            .operations
            .keys()
            .filter(|id| {
                id.process_id == command.process_id
                    && id.task_id == command.parent.task_id
                    && id.generation == command.parent.generation
            })
            .map(|id| id.sequence)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(ContainerError::SequenceOverflow)?;
        Ok(Some((
            StateChange {
                execution: command.parent.clone(),
                expected_revision: parent.revision,
                from: ExecutionState::Waiting,
                to: ExecutionState::Completed,
            },
            OperationIntent {
                id: OperationId {
                    process_id: command.process_id.clone(),
                    task_id: command.parent.task_id.clone(),
                    generation: command.parent.generation,
                    sequence,
                },
                kind: OperationKind::PropagateTask,
            },
        )))
    }
}

/// The existing-generation observation for one task's admission slot: the
/// state of its latest generation. `Created` never ran (`Unstarted`), so a
/// rewound-and-recreated child is readmitted rather than duplicated.
fn existing_generation(snapshot: &AggregateSnapshot, task_id: &str) -> ExistingGeneration {
    let Some(latest) = latest_execution(snapshot, task_id) else {
        return ExistingGeneration::Absent;
    };
    match snapshot.executions[&latest].state {
        ExecutionState::Created => ExistingGeneration::Unstarted,
        ExecutionState::Ready | ExecutionState::Running | ExecutionState::Waiting => {
            ExistingGeneration::Active
        }
        _ => ExistingGeneration::Completed,
    }
}

fn latest_execution(snapshot: &AggregateSnapshot, task_id: &str) -> Option<ExecutionId> {
    snapshot
        .executions
        .keys()
        .filter(|id| id.task_id == task_id)
        .max_by_key(|id| id.generation)
        .cloned()
}

fn next_generation(snapshot: &AggregateSnapshot, task_id: &str) -> Option<u64> {
    snapshot
        .executions
        .keys()
        .filter(|id| id.task_id == task_id)
        .map(|id| id.generation)
        .max()
        .map_or(Some(0), |value| value.checked_add(1))
}

/// Turn symbolic predecessor references into execution ids: the container
/// itself, or a sibling processed earlier in the same command (its resolved
/// id, created or readmitted).
fn resolve_predecessors(
    references: &[ContainerPredecessor],
    source: &ExecutionId,
    resolved: &[ExecutionId],
    current: usize,
) -> Result<Vec<ExecutionId>, ContainerError> {
    references
        .iter()
        .map(|reference| match *reference {
            ContainerPredecessor::Source => Ok(source.clone()),
            ContainerPredecessor::Sibling(index) => {
                if index >= current {
                    return Err(ContainerError::ForwardSibling(index));
                }
                resolved
                    .get(index)
                    .cloned()
                    .ok_or(ContainerError::ForwardSibling(index))
            }
        })
        .collect()
}

/// The admission operation of one child, when its plan needs one.
fn child_operation(
    command: &ContainerFanoutCommand,
    execution: &ExecutionId,
    plan: ContainerChildPlan,
) -> Result<Option<OperationIntent>, ContainerError> {
    let kind = match plan {
        ContainerChildPlan::Queued => return Ok(None),
        ContainerChildPlan::Entry => OperationKind::ExecuteTask,
        ContainerChildPlan::Skip => OperationKind::SkipTask,
    };
    Ok(Some(OperationIntent {
        id: OperationId {
            process_id: command.process_id.clone(),
            task_id: execution.task_id.clone(),
            generation: execution.generation,
            sequence: 0,
        },
        kind,
    }))
}

fn child_variables(
    writes: &mut Vec<VariableWrite>,
    execution: &ExecutionId,
    child: &ContainerChild,
) {
    writes.push(VariableWrite {
        scope: execution.clone(),
        name: "auto".to_string(),
        value: if child.auto { "1" } else { "0" }.to_string(),
    });
    for (name, value) in &child.vars {
        writes.push(VariableWrite {
            scope: execution.clone(),
            name: name.clone(),
            value: value.clone(),
        });
    }
}

/// A readmitted child is `Created`; the admission moves it to `Ready`. An
/// unexpected state is refused rather than forced.
fn readmit_change(
    snapshot: &AggregateSnapshot,
    execution: &ExecutionId,
) -> Result<Option<StateChange>, ContainerError> {
    let existing = snapshot
        .executions
        .get(execution)
        .ok_or_else(|| ContainerError::ExecutionNotFound(execution.clone()))?;
    if existing.state != ExecutionState::Created {
        return Err(ContainerError::ReadmitNotCreated {
            execution: execution.clone(),
            state: existing.state,
        });
    }
    Ok(Some(StateChange {
        execution: execution.clone(),
        expected_revision: existing.revision,
        from: ExecutionState::Created,
        to: ExecutionState::Ready,
    }))
}

fn fanout_outbox(command: &ContainerFanoutCommand, outcome: &str) -> OutboxIntent {
    OutboxIntent {
        key: format!(
            "fanout:{}:{}:{}:{}",
            command.process_id,
            command.source.task_id,
            command.source.generation,
            command.source_operation.id.sequence,
        ),
        topic: format!("fanout.{outcome}"),
        payload: format!("{}:{}", command.source.task_id, command.source.generation),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ContainerError {
    WrongAggregate,
    ExecutionNotFound(ExecutionId),
    SourceNotContainer {
        source: ExecutionId,
        state: ExecutionState,
    },
    SourceOperationTargetsDifferentExecution,
    EmptyChildTaskId,
    ForwardSibling(usize),
    ReadmitNotCreated {
        execution: ExecutionId,
        state: ExecutionState,
    },
    GraphVersionConflict {
        graph: u64,
        snapshot: u64,
    },
    GenerationOverflow,
    SequenceOverflow,
    InvalidPlan(String),
}
impl fmt::Display for ContainerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "container error: {self:?}")
    }
}
impl std::error::Error for ContainerError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::{AggregateVersion, ClaimToken, ExecutionSnapshot};
    use std::collections::BTreeMap;

    fn id(task: &str, generation: u64) -> ExecutionId {
        ExecutionId::new(task, generation).unwrap()
    }
    fn node(name: &str) -> NodeId {
        NodeId::parse(name).unwrap()
    }
    fn claim() -> ClaimToken {
        ClaimToken {
            owner: "r1".into(),
            fence: 2,
            lease_until_ms: i64::MAX,
            heartbeat_revision: 0,
        }
    }
    fn commit(task: &str, sequence: u64) -> OperationCommit {
        OperationCommit {
            id: OperationId {
                process_id: "p1".into(),
                task_id: task.into(),
                generation: 0,
                sequence,
            },
            claim: claim(),
        }
    }
    fn execution(
        task: &str,
        generation: u64,
        state: ExecutionState,
    ) -> (ExecutionId, ExecutionSnapshot) {
        (
            id(task, generation),
            ExecutionSnapshot {
                model_node: node(task),
                parent: None,
                predecessors: vec![],
                state,
                revision: 2,
            },
        )
    }
    fn snapshot(executions: Vec<(ExecutionId, ExecutionSnapshot)>) -> AggregateSnapshot {
        AggregateSnapshot {
            process_id: "p1".into(),
            version: AggregateVersion {
                graph: 1,
                process: 3,
                operations: 2,
            },
            executions: HashMap::from_iter(executions),
            variables: BTreeMap::new(),
            operations: HashMap::new(),
            outbox: HashMap::new(),
        }
    }
    fn child(
        task: &str,
        plan: ContainerChildPlan,
        predecessors: Vec<ContainerPredecessor>,
    ) -> ContainerChild {
        ContainerChild {
            task_id: task.into(),
            model_node: node(task),
            plan,
            predecessors,
            vars: Vec::new(),
            auto: true,
        }
    }
    fn fanout_command(
        source: &str,
        source_state: ExecutionState,
        children: Vec<ContainerChild>,
    ) -> (AggregateSnapshot, ContainerFanoutCommand) {
        let snapshot = snapshot(vec![
            execution(source, 0, source_state),
            execution("$", 0, ExecutionState::Completed),
        ]);
        let command = ContainerFanoutCommand {
            process_id: "p1".into(),
            source: id(source, 0),
            source_operation: commit(source, 4),
            children,
            inputs: Vec::new(),
        };
        (snapshot, command)
    }

    #[test]
    fn fanout_parks_the_source_and_admits_entry_and_skip_children() {
        let (snapshot, command) = fanout_command(
            "step1",
            ExecutionState::Running,
            vec![
                child(
                    "b1",
                    ContainerChildPlan::Entry,
                    vec![ContainerPredecessor::Source],
                ),
                child(
                    "b2",
                    ContainerChildPlan::Skip,
                    vec![ContainerPredecessor::Source],
                ),
            ],
        );
        let plan = ContainerPlanner::plan_fanout(&snapshot, &command).unwrap();
        assert_eq!(plan.execution_creates.len(), 2);
        assert!(
            plan.execution_creates
                .iter()
                .all(|create| create.parent == Some(id("step1", 0)))
        );
        let entry = plan
            .execution_creates
            .iter()
            .find(|create| create.execution.task_id == "b1")
            .unwrap();
        assert_eq!(entry.initial_state, ExecutionState::Ready);
        let skip = plan
            .execution_creates
            .iter()
            .find(|create| create.execution.task_id == "b2")
            .unwrap();
        assert_eq!(skip.initial_state, ExecutionState::Ready);
        assert_eq!(plan.operations.len(), 2);
        assert!(
            plan.operations
                .iter()
                .any(|op| op.id.task_id == "b1" && op.kind == OperationKind::ExecuteTask)
        );
        assert!(
            plan.operations
                .iter()
                .any(|op| op.id.task_id == "b2" && op.kind == OperationKind::SkipTask)
        );
        assert_eq!(plan.state_changes[0].to, ExecutionState::Waiting);
        // The auto decision is durable state for the created children.
        assert_eq!(plan.variable_writes.len(), 2);
    }

    #[test]
    fn fanout_queues_chain_successors_behind_the_entry() {
        let (snapshot, command) = fanout_command(
            "b1",
            ExecutionState::Running,
            vec![
                child(
                    "s1",
                    ContainerChildPlan::Entry,
                    vec![ContainerPredecessor::Source],
                ),
                child(
                    "s2",
                    ContainerChildPlan::Queued,
                    vec![ContainerPredecessor::Sibling(0)],
                ),
            ],
        );
        let plan = ContainerPlanner::plan_fanout(&snapshot, &command).unwrap();
        assert_eq!(plan.execution_creates.len(), 2);
        let queued = plan
            .execution_creates
            .iter()
            .find(|create| create.execution.task_id == "s2")
            .unwrap();
        assert_eq!(queued.initial_state, ExecutionState::Created);
        assert_eq!(queued.predecessors, vec![id("s1", 0)]);
        // Only the entry receives an operation; propagation admits the rest.
        assert_eq!(plan.operations.len(), 1);
        assert_eq!(plan.operations[0].kind, OperationKind::ExecuteTask);
    }

    #[test]
    fn empty_container_completes_and_propagates_in_the_same_hop() {
        let (snapshot, command) = fanout_command("b1", ExecutionState::Running, vec![]);
        let plan = ContainerPlanner::plan_fanout(&snapshot, &command).unwrap();
        assert!(plan.execution_creates.is_empty());
        assert_eq!(plan.state_changes[0].to, ExecutionState::Completed);
        assert_eq!(plan.operations[0].kind, OperationKind::PropagateTask);
    }

    #[test]
    fn a_rewound_created_child_is_readmitted_not_duplicated() {
        let mut snapshot = snapshot(vec![
            execution("step1", 1, ExecutionState::Running),
            execution("b1", 0, ExecutionState::Cancelled),
            // The Cancel rewind recreated b1 at generation 1; it never ran.
            (
                id("b1", 1),
                ExecutionSnapshot {
                    model_node: node("b1"),
                    parent: Some(id("step1", 1)),
                    predecessors: vec![id("step1", 1)],
                    state: ExecutionState::Created,
                    revision: 0,
                },
            ),
        ]);
        snapshot.version.graph = 2;
        let command = ContainerFanoutCommand {
            process_id: "p1".into(),
            source: id("step1", 1),
            source_operation: OperationCommit {
                id: OperationId {
                    process_id: "p1".into(),
                    task_id: "step1".into(),
                    generation: 1,
                    sequence: 4,
                },
                claim: claim(),
            },
            children: vec![child(
                "b1",
                ContainerChildPlan::Entry,
                vec![ContainerPredecessor::Source],
            )],
            inputs: Vec::new(),
        };
        let plan = ContainerPlanner::plan_fanout(&snapshot, &command).unwrap();
        assert!(plan.execution_creates.is_empty(), "no duplicate generation");
        assert_eq!(plan.state_changes.len(), 2);
        assert_eq!(plan.state_changes[1].execution, id("b1", 1));
        assert_eq!(plan.state_changes[1].from, ExecutionState::Created);
        assert_eq!(plan.state_changes[1].to, ExecutionState::Ready);
        assert_eq!(
            plan.operations[0].id,
            OperationId {
                process_id: "p1".into(),
                task_id: "b1".into(),
                generation: 1,
                sequence: 0,
            }
        );
    }

    #[test]
    fn an_active_child_is_ignored_not_doubled() {
        let snapshot = snapshot(vec![
            execution("step1", 0, ExecutionState::Running),
            (
                id("b1", 0),
                ExecutionSnapshot {
                    model_node: node("b1"),
                    parent: Some(id("step1", 0)),
                    predecessors: vec![id("step1", 0)],
                    state: ExecutionState::Running,
                    revision: 1,
                },
            ),
        ]);
        let command = ContainerFanoutCommand {
            process_id: "p1".into(),
            source: id("step1", 0),
            source_operation: commit("step1", 4),
            children: vec![child(
                "b1",
                ContainerChildPlan::Entry,
                vec![ContainerPredecessor::Source],
            )],
            inputs: Vec::new(),
        };
        let plan = ContainerPlanner::plan_fanout(&snapshot, &command).unwrap();
        assert!(plan.execution_creates.is_empty());
        assert!(plan.operations.is_empty());
    }

    #[test]
    fn a_completed_child_reruns_as_a_fresh_generation() {
        let snapshot = snapshot(vec![
            execution("step1", 1, ExecutionState::Running),
            execution("b1", 0, ExecutionState::Completed),
        ]);
        let command = ContainerFanoutCommand {
            process_id: "p1".into(),
            source: id("step1", 1),
            source_operation: OperationCommit {
                id: OperationId {
                    process_id: "p1".into(),
                    task_id: "step1".into(),
                    generation: 1,
                    sequence: 2,
                },
                claim: claim(),
            },
            children: vec![child(
                "b1",
                ContainerChildPlan::Entry,
                vec![ContainerPredecessor::Source],
            )],
            inputs: Vec::new(),
        };
        let plan = ContainerPlanner::plan_fanout(&snapshot, &command).unwrap();
        let create = &plan.execution_creates[0];
        assert_eq!(create.execution, id("b1", 1), "next generation allocated");
    }

    #[test]
    fn a_terminal_source_is_not_a_container() {
        let (snapshot, command) = fanout_command(
            "step1",
            ExecutionState::Completed,
            vec![child(
                "b1",
                ContainerChildPlan::Entry,
                vec![ContainerPredecessor::Source],
            )],
        );
        let error = ContainerPlanner::plan_fanout(&snapshot, &command).unwrap_err();
        assert!(matches!(error, ContainerError::SourceNotContainer { .. }));
    }

    #[test]
    fn join_completes_the_parent_when_every_child_is_terminal() {
        let mut b = super::super::ExecutionGraphBuilder::new("p1", 1).unwrap();
        for (task, generation) in [("step1", 0u64), ("b1", 0), ("b2", 0)] {
            b.add_node(super::super::ExecutionNode {
                id: id(task, generation),
                model_node: node(task),
                state: ExecutionState::Created,
                revision: 0,
            })
            .unwrap();
        }
        b.set_root(id("step1", 0)).unwrap();
        b.add_child(id("step1", 0), id("b1", 0)).unwrap();
        b.add_child(id("step1", 0), id("b2", 0)).unwrap();
        let graph = b.build().unwrap();
        let snapshot = snapshot(vec![
            (
                id("step1", 0),
                ExecutionSnapshot {
                    model_node: node("step1"),
                    parent: None,
                    predecessors: vec![],
                    state: ExecutionState::Waiting,
                    revision: 3,
                },
            ),
            (
                id("b1", 0),
                ExecutionSnapshot {
                    model_node: node("b1"),
                    parent: Some(id("step1", 0)),
                    predecessors: vec![id("step1", 0)],
                    state: ExecutionState::Completed,
                    revision: 5,
                },
            ),
            (
                id("b2", 0),
                ExecutionSnapshot {
                    model_node: node("b2"),
                    parent: Some(id("step1", 0)),
                    predecessors: vec![id("step1", 0)],
                    state: ExecutionState::Skipped,
                    revision: 4,
                },
            ),
        ]);
        let command = ContainerJoinCommand {
            process_id: "p1".into(),
            parent: id("step1", 0),
        };
        let (change, operation) = ContainerPlanner::plan_join(&graph, &snapshot, &command)
            .unwrap()
            .unwrap();
        assert_eq!(change.execution, id("step1", 0));
        assert_eq!(change.from, ExecutionState::Waiting);
        assert_eq!(change.to, ExecutionState::Completed);
        assert_eq!(operation.kind, OperationKind::PropagateTask);
        assert_eq!(operation.id.task_id, "step1");
        assert_eq!(operation.id.sequence, 1);
    }

    #[test]
    fn join_waits_for_running_children_and_never_fires_on_an_aborted_one() {
        let graph = {
            let mut b = super::super::ExecutionGraphBuilder::new("p1", 1).unwrap();
            for task in ["step1", "b1", "b2"] {
                b.add_node(super::super::ExecutionNode {
                    id: id(task, 0),
                    model_node: node(task),
                    state: ExecutionState::Created,
                    revision: 0,
                })
                .unwrap();
            }
            b.set_root(id("step1", 0)).unwrap();
            b.add_child(id("step1", 0), id("b1", 0)).unwrap();
            b.add_child(id("step1", 0), id("b2", 0)).unwrap();
            b.build().unwrap()
        };
        let command = ContainerJoinCommand {
            process_id: "p1".into(),
            parent: id("step1", 0),
        };
        let waiting_child = |b1: ExecutionState, b2: ExecutionState| {
            snapshot(vec![
                (
                    id("step1", 0),
                    ExecutionSnapshot {
                        model_node: node("step1"),
                        parent: None,
                        predecessors: vec![],
                        state: ExecutionState::Waiting,
                        revision: 3,
                    },
                ),
                (
                    id("b1", 0),
                    ExecutionSnapshot {
                        model_node: node("b1"),
                        parent: Some(id("step1", 0)),
                        predecessors: vec![id("step1", 0)],
                        state: b1,
                        revision: 1,
                    },
                ),
                (
                    id("b2", 0),
                    ExecutionSnapshot {
                        model_node: node("b2"),
                        parent: Some(id("step1", 0)),
                        predecessors: vec![id("step1", 0)],
                        state: b2,
                        revision: 1,
                    },
                ),
            ])
        };
        assert!(
            ContainerPlanner::plan_join(
                &graph,
                &waiting_child(ExecutionState::Running, ExecutionState::Completed),
                &command
            )
            .unwrap()
            .is_none()
        );
        // An aborted child leaves the outcome to the abort walk.
        assert!(
            ContainerPlanner::plan_join(
                &graph,
                &waiting_child(ExecutionState::Completed, ExecutionState::Aborted),
                &command
            )
            .unwrap()
            .is_none()
        );
    }
}
