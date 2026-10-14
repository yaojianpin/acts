use crate::scheduler::core::{
    commit::aggregate::{AggregateSnapshot, ExecutionSnapshot},
    model::{
        execution_graph::{ExecutionGraph, ExecutionId, ExecutionState},
        operation::{OperationId, OperationKind},
        transition_plan::{
            ExecutionCreate, OperationCommit, OperationIntent, OutboxIntent, TransitionPlan,
            VariableWrite,
        },
    },
};

use std::collections::{HashMap, VecDeque};
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReenterCommand {
    pub(crate) process_id: String,
    /// The terminal execution requesting re-entry, normally a Backed task.
    pub(crate) source: ExecutionId,
    /// Stable logical task identity to instantiate again.
    pub(crate) target_task_id: String,
    pub(crate) source_operation: OperationCommit,
}

pub(crate) struct GenerationPlanner;

impl GenerationPlanner {
    /// Re-enter one task with a fresh generation. The re-entered execution's
    /// whole forward closure is recreated alongside it (new generations in
    /// breadth-first order, predecessor and parent edges remapped to the new
    /// generations), because the old forward executions are dead on this
    /// path. A Cancel re-enters the terminal source itself; a Back re-enters
    /// the step the client named (`target_task_id` earlier on the path — the
    /// backed source must lie inside the redo closure). Only the re-entered
    /// root receives an `ExecuteTask` operation - its completion propagates
    /// into the recreated chain. `graph` is the snapshot's execution graph
    /// (rebuilt by the caller).
    pub(crate) fn plan_reentry(
        graph: &ExecutionGraph,
        snapshot: &AggregateSnapshot,
        command: &ReenterCommand,
    ) -> Result<TransitionPlan, GenerationError> {
        if snapshot.process_id != command.process_id {
            return Err(GenerationError::WrongAggregate);
        }
        let source = snapshot
            .executions
            .get(&command.source)
            .ok_or_else(|| GenerationError::ExecutionNotFound(command.source.clone()))?;
        if !matches!(
            source.state,
            ExecutionState::Backed | ExecutionState::Cancelled
        ) {
            return Err(GenerationError::SourceNotReenterable {
                source: command.source.clone(),
                state: source.state,
            });
        }
        let operation = &command.source_operation.id;
        if operation.process_id != command.process_id
            || operation.task_id != command.source.task_id
            || operation.generation != command.source.generation
        {
            return Err(GenerationError::SourceOperationTargetsDifferentExecution);
        }
        if command.target_task_id.trim().is_empty() {
            return Err(GenerationError::EmptyTargetTaskId);
        }
        // The closure roots at the re-entered task: a Cancel re-enters the
        // terminal source itself; a Back re-enters an EARLIER step named by
        // the client (`to`), so the root is that task's latest generation
        // and the backed source must lie inside the closure it spawns (the
        // redo re-runs everything downstream of the target, the source
        // included).
        let root = if command.target_task_id == command.source.task_id {
            command.source.clone()
        } else {
            snapshot
                .executions
                .keys()
                .filter(|id| id.task_id == command.target_task_id)
                .max_by_key(|id| id.generation)
                .cloned()
                .ok_or_else(|| {
                    GenerationError::TargetNeverInstantiated(command.target_task_id.clone())
                })?
        };
        // Forward closure of the re-entered source in breadth-first order
        // (edges always point forward, so BFS is a topological order): the
        // source and every execution downstream of it are dead on this
        // path and must be recreated as fresh generations.
        let mut order: Vec<ExecutionId> = Vec::new();
        let mut seen: HashMap<ExecutionId, ()> = HashMap::new();
        let mut queue = VecDeque::new();
        seen.insert(root.clone(), ());
        order.push(root.clone());
        queue.push_back(root.clone());
        while let Some(current) = queue.pop_front() {
            for successor in graph.successors(&current) {
                if seen.insert(successor.clone(), ()).is_some() {
                    continue;
                }
                order.push(successor.clone());
                queue.push_back(successor.clone());
            }
        }
        if !seen.contains_key(&command.source) {
            return Err(GenerationError::SourceOutsideRedoClosure {
                source: command.source.clone(),
                target: command.target_task_id.clone(),
            });
        }

        // One fresh generation per closed-over task, remapping every
        // predecessor edge onto the new generations (edges from outside the
        // closure - typically the completed root - keep pointing at
        // themselves through the identity map below).
        let mut remap: HashMap<ExecutionId, ExecutionId> = HashMap::new();
        let mut closure: Vec<(ExecutionId, &ExecutionSnapshot, ExecutionId)> =
            Vec::with_capacity(order.len());
        for old_id in &order {
            let old = &snapshot.executions[old_id];
            let generation = snapshot
                .executions
                .keys()
                .filter(|id| id.task_id == old_id.task_id)
                .map(|id| id.generation)
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or(GenerationError::GenerationOverflow)?;
            let new_id = ExecutionId::new(old_id.task_id.clone(), generation)
                .map_err(|_| GenerationError::EmptyTargetTaskId)?;
            remap.insert(old_id.clone(), new_id.clone());
            closure.push((old_id.clone(), old, new_id));
        }
        // The re-entered root generation is admitted immediately (`Ready`,
        // carried by its `ExecuteTask` operation); the recreated downstream
        // starts `Created`, so ordinary propagation admits each of them in
        // turn as their predecessors settle.
        let next = remap
            .get(&root)
            .cloned()
            .ok_or_else(|| GenerationError::TargetNeverInstantiated(root.task_id.clone()))?;
        let execution_creates: Vec<ExecutionCreate> = closure
            .iter()
            .map(|(old_id, old, new_id)| ExecutionCreate {
                execution: new_id.clone(),
                model_node: old.model_node.clone(),
                // A nested closure's children belong to the re-entered
                // generation, not the dead one: the parent edge remaps like
                // the predecessor edges (outside the closure - the root -
                // the old parent stays).
                parent: old
                    .parent
                    .as_ref()
                    .map(|parent| remap.get(parent).cloned().unwrap_or_else(|| parent.clone())),
                predecessors: snapshot.executions[old_id]
                    .predecessors
                    .iter()
                    .map(|predecessor| {
                        remap
                            .get(predecessor)
                            .cloned()
                            .unwrap_or_else(|| predecessor.clone())
                    })
                    .collect(),
                initial_state: if *new_id == next {
                    ExecutionState::Ready
                } else {
                    ExecutionState::Created
                },
            })
            .collect();
        // The auto/waiting decision is durable per-execution state: recreated
        // generations inherit it from the generation they replace, or default
        // to self-completing when the replaced generation predates the
        // variable.
        let variable_writes: Vec<VariableWrite> = closure
            .iter()
            .map(|(old_id, _, new_id)| VariableWrite {
                scope: new_id.clone(),
                name: "auto".to_string(),
                value: snapshot
                    .variables
                    .get(&((*old_id).clone(), "auto".to_string()))
                    .cloned()
                    .unwrap_or_else(|| "1".to_string()),
            })
            .collect();

        // The re-entered source generation is the one that runs again; the
        // recreated downstream waits for its propagation.
        let next_operation = OperationId {
            process_id: command.process_id.clone(),
            task_id: next.task_id.clone(),
            generation: next.generation,
            sequence: 0,
        };
        let plan = TransitionPlan {
            process_id: command.process_id.clone(),
            expected: snapshot.version.clone(),
            execution_creates,
            state_changes: Vec::new(),
            variable_writes,
            source_operation: Some(command.source_operation.clone()),
            operations: vec![OperationIntent {
                id: next_operation,
                kind: OperationKind::ExecuteTask,
            }],
            outbox: vec![OutboxIntent {
                key: format!(
                    "reenter:{}:{}:{}:{}",
                    command.process_id,
                    next.task_id,
                    next.generation,
                    command.source_operation.id.sequence,
                ),
                topic: "task.reentered".into(),
                payload: format!("{}:{}", next.task_id, next.generation),
            }],
        };
        plan.validate()
            .map_err(|error| GenerationError::InvalidPlan(error.to_string()))?;
        Ok(plan)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum GenerationError {
    WrongAggregate,
    ExecutionNotFound(ExecutionId),
    SourceNotReenterable {
        source: ExecutionId,
        state: ExecutionState,
    },
    SourceOperationTargetsDifferentExecution,
    EmptyTargetTaskId,
    TargetNeverInstantiated(String),
    /// A Back named a `to` step that is not on the source's path: the redo
    /// closure from that step would not carry the backed source.
    SourceOutsideRedoClosure {
        source: ExecutionId,
        target: String,
    },
    GenerationOverflow,
    InvalidPlan(String),
}
impl fmt::Display for GenerationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "generation error: {self:?}")
    }
}
impl std::error::Error for GenerationError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::core::commit::aggregate::ExecutionSnapshot;
    use crate::scheduler::core::model::model_graph::NodeId;
    use crate::scheduler::core::model::operation::ClaimToken;
    use crate::scheduler::core::model::transition_plan::AggregateVersion;
    use std::collections::{BTreeMap, HashMap};

    fn id(task: &str, generation: u64) -> ExecutionId {
        ExecutionId::new(task, generation).unwrap()
    }
    fn snapshot() -> AggregateSnapshot {
        AggregateSnapshot {
            process_id: "p1".into(),
            version: AggregateVersion {
                graph: 3,
                process: 8,
                operations: 6,
            },
            executions: HashMap::from([
                (
                    id("source", 1),
                    ExecutionSnapshot {
                        model_node: NodeId::parse("source").unwrap(),
                        parent: None,
                        predecessors: vec![],
                        state: ExecutionState::Backed,
                        revision: 4,
                    },
                ),
                (
                    id("target", 0),
                    ExecutionSnapshot {
                        model_node: NodeId::parse("target-node").unwrap(),
                        parent: None,
                        predecessors: vec![],
                        state: ExecutionState::Completed,
                        revision: 3,
                    },
                ),
                (
                    id("target", 1),
                    ExecutionSnapshot {
                        model_node: NodeId::parse("target-node").unwrap(),
                        parent: None,
                        predecessors: vec![id("source", 1)],
                        state: ExecutionState::Completed,
                        revision: 3,
                    },
                ),
            ]),
            variables: BTreeMap::new(),
            operations: HashMap::new(),
            outbox: HashMap::new(),
        }
    }
    /// The execution graph of [`snapshot`]: source gen1 -> target gen1.
    fn graph() -> ExecutionGraph {
        use crate::scheduler::core::model::execution_graph::ExecutionGraphBuilder;
        use crate::scheduler::core::model::execution_graph::ExecutionNode;
        let snapshot = snapshot();
        let mut builder =
            ExecutionGraphBuilder::new(&snapshot.process_id, snapshot.version.graph).unwrap();
        for (exec_id, execution) in &snapshot.executions {
            builder
                .add_node(ExecutionNode {
                    id: exec_id.clone(),
                    model_node: execution.model_node.clone(),
                    state: execution.state,
                    revision: execution.revision,
                })
                .unwrap();
        }
        builder.set_root(id("source", 1)).unwrap();
        builder.add_child(id("source", 1), id("target", 0)).unwrap();
        builder.add_child(id("source", 1), id("target", 1)).unwrap();
        // The propagation edge mirrors the target's predecessor list: the
        // live generation hangs off the cancelled source generation.
        builder
            .add_successor(id("source", 1), id("target", 1))
            .unwrap();
        builder.build().unwrap()
    }
    fn command() -> ReenterCommand {
        ReenterCommand {
            process_id: "p1".into(),
            source: id("source", 1),
            // Cancel re-enters the source itself; the target names the
            // re-entered task.
            target_task_id: "source".into(),
            source_operation: OperationCommit {
                id: OperationId {
                    process_id: "p1".into(),
                    task_id: "source".into(),
                    generation: 1,
                    sequence: 8,
                },
                claim: ClaimToken {
                    owner: "runtime-a".into(),
                    fence: 7,
                    lease_until_ms: i64::MAX,
                    heartbeat_revision: 0,
                },
            },
        }
    }

    #[test]
    fn reentry_creates_new_generation_and_never_mutates_history() {
        let plan = GenerationPlanner::plan_reentry(&graph(), &snapshot(), &command()).unwrap();
        assert!(plan.state_changes.is_empty());
        // The re-entered source generation runs again; the downstream target
        // is recreated one generation up, waiting for the source's
        // propagation.
        let created: Vec<ExecutionId> = plan
            .execution_creates
            .iter()
            .map(|create| create.execution.clone())
            .collect();
        assert_eq!(created, vec![id("source", 2), id("target", 2)]);
        assert_eq!(
            plan.execution_creates[0].initial_state,
            ExecutionState::Ready
        );
        assert_eq!(plan.operations[0].id.task_id, "source");
        assert_eq!(plan.operations[0].id.generation, 2);
        let target = plan
            .execution_creates
            .iter()
            .find(|create| create.execution == id("target", 2))
            .unwrap();
        assert_eq!(target.predecessors, vec![id("source", 2)]);
    }

    #[test]
    fn non_reenterable_source_cannot_reenter() {
        let mut snapshot = snapshot();
        snapshot.executions.get_mut(&id("source", 1)).unwrap().state = ExecutionState::Waiting;
        assert!(matches!(
            GenerationPlanner::plan_reentry(&graph(), &snapshot, &command()),
            Err(GenerationError::SourceNotReenterable { .. })
        ));
    }

    #[test]
    fn cancelled_source_reenters_and_recreates_the_forward_closure() {
        let mut snapshot = snapshot();
        snapshot.executions.get_mut(&id("source", 1)).unwrap().state = ExecutionState::Cancelled;
        // The old target generation was skipped by the cascade that followed
        // the cancellation.
        snapshot.executions.get_mut(&id("target", 1)).unwrap().state = ExecutionState::Skipped;
        let plan = GenerationPlanner::plan_reentry(&graph(), &snapshot, &command()).unwrap();
        // Both the target and the forward source generation are recreated.
        let created: Vec<ExecutionId> = plan
            .execution_creates
            .iter()
            .map(|create| create.execution.clone())
            .collect();
        assert_eq!(created, vec![id("source", 2), id("target", 2)]);
        // The recreated source carries an ExecuteTask; the recreated target
        // waits for the source's propagation.
        assert_eq!(plan.operations.len(), 1);
        assert_eq!(plan.operations[0].id.task_id, "source");
        assert_eq!(plan.operations[0].id.generation, 2);
        // The recreated target's predecessor edge points at the recreated
        // source, not at the dead generation.
        let target = plan
            .execution_creates
            .iter()
            .find(|create| create.execution == id("target", 2))
            .unwrap();
        assert_eq!(target.predecessors, vec![id("source", 2)]);
    }

    #[test]
    fn back_reenters_the_named_step_and_carries_the_backed_source() {
        // The source is the Backed downstream execution; the client named
        // `target` as the redo point. The closure roots at the target's
        // latest generation and must carry the source with it.
        let snapshot = AggregateSnapshot {
            process_id: "p1".into(),
            version: AggregateVersion {
                graph: 3,
                process: 8,
                operations: 6,
            },
            executions: HashMap::from([
                (
                    id("target", 1),
                    ExecutionSnapshot {
                        model_node: NodeId::parse("target-node").unwrap(),
                        parent: None,
                        predecessors: vec![],
                        state: ExecutionState::Completed,
                        revision: 3,
                    },
                ),
                (
                    id("source", 1),
                    ExecutionSnapshot {
                        model_node: NodeId::parse("source").unwrap(),
                        parent: None,
                        predecessors: vec![id("target", 1)],
                        state: ExecutionState::Backed,
                        revision: 4,
                    },
                ),
            ]),
            variables: BTreeMap::new(),
            operations: HashMap::new(),
            outbox: HashMap::new(),
        };
        let graph = {
            let mut builder =
                crate::scheduler::core::model::execution_graph::ExecutionGraphBuilder::new("p1", 3)
                    .unwrap();
            for (exec_id, execution) in &snapshot.executions {
                builder
                    .add_node(
                        crate::scheduler::core::model::execution_graph::ExecutionNode {
                            id: exec_id.clone(),
                            model_node: execution.model_node.clone(),
                            state: execution.state,
                            revision: execution.revision,
                        },
                    )
                    .unwrap();
            }
            builder.set_root(id("target", 1)).unwrap();
            builder.add_child(id("target", 1), id("source", 1)).unwrap();
            builder
                .add_successor(id("target", 1), id("source", 1))
                .unwrap();
            builder.build().unwrap()
        };
        let mut command = command();
        command.target_task_id = "target".into();
        let plan = GenerationPlanner::plan_reentry(&graph, &snapshot, &command).unwrap();
        let created: Vec<ExecutionId> = plan
            .execution_creates
            .iter()
            .map(|create| create.execution.clone())
            .collect();
        // The redo starts at the target's next generation; the backed source
        // is recreated behind it (the redo re-runs everything downstream).
        assert_eq!(created, vec![id("target", 2), id("source", 2)]);
        assert_eq!(
            plan.execution_creates[0].initial_state,
            ExecutionState::Ready,
            "the named redo step runs again immediately"
        );
        assert_eq!(plan.operations[0].id.task_id, "target");
        assert_eq!(plan.operations[0].id.generation, 2);
        let source = plan
            .execution_creates
            .iter()
            .find(|create| create.execution == id("source", 2))
            .unwrap();
        assert_eq!(source.initial_state, ExecutionState::Created);
        assert_eq!(source.predecessors, vec![id("target", 2)]);
    }

    #[test]
    fn back_to_a_step_outside_the_redo_closure_is_refused() {
        // A source with no path from the named step cannot be a redo. The
        // graph stays connected: one root with two children, and the named
        // `target` spawns a closure that misses the backed `stray`.
        let snapshot = AggregateSnapshot {
            process_id: "p1".into(),
            version: AggregateVersion {
                graph: 4,
                process: 9,
                operations: 7,
            },
            executions: HashMap::from([
                (
                    id("root", 0),
                    ExecutionSnapshot {
                        model_node: NodeId::parse("root").unwrap(),
                        parent: None,
                        predecessors: vec![],
                        state: ExecutionState::Completed,
                        revision: 1,
                    },
                ),
                (
                    id("target", 1),
                    ExecutionSnapshot {
                        model_node: NodeId::parse("target-node").unwrap(),
                        parent: Some(id("root", 0)),
                        predecessors: vec![],
                        state: ExecutionState::Completed,
                        revision: 3,
                    },
                ),
                (
                    id("stray", 0),
                    ExecutionSnapshot {
                        model_node: NodeId::parse("stray").unwrap(),
                        parent: Some(id("root", 0)),
                        predecessors: vec![],
                        state: ExecutionState::Backed,
                        revision: 1,
                    },
                ),
            ]),
            variables: BTreeMap::new(),
            operations: HashMap::new(),
            outbox: HashMap::new(),
        };
        let graph = {
            let mut builder =
                crate::scheduler::core::model::execution_graph::ExecutionGraphBuilder::new("p1", 4)
                    .unwrap();
            for (exec_id, execution) in &snapshot.executions {
                builder
                    .add_node(
                        crate::scheduler::core::model::execution_graph::ExecutionNode {
                            id: exec_id.clone(),
                            model_node: execution.model_node.clone(),
                            state: execution.state,
                            revision: execution.revision,
                        },
                    )
                    .unwrap();
            }
            builder.set_root(id("root", 0)).unwrap();
            builder.add_child(id("root", 0), id("target", 1)).unwrap();
            builder.add_child(id("root", 0), id("stray", 0)).unwrap();
            builder.build().unwrap()
        };
        let command = ReenterCommand {
            process_id: "p1".into(),
            source: id("stray", 0),
            target_task_id: "target".into(),
            source_operation: OperationCommit {
                id: OperationId {
                    process_id: "p1".into(),
                    task_id: "stray".into(),
                    generation: 0,
                    sequence: 1,
                },
                claim: ClaimToken {
                    owner: "runtime-a".into(),
                    fence: 7,
                    lease_until_ms: i64::MAX,
                    heartbeat_revision: 0,
                },
            },
        };
        assert!(matches!(
            GenerationPlanner::plan_reentry(&graph, &snapshot, &command),
            Err(GenerationError::SourceOutsideRedoClosure { .. })
        ));
    }
}
