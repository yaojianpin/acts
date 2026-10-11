use super::{
    AggregateSnapshot, ExecutionCreate, ExecutionGraph, ExecutionId, ExecutionSnapshot,
    ExecutionState, NodeId, OperationCommit, OperationId, OperationIntent, OperationKind,
    OutboxIntent, TransitionPlan, VariableWrite,
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
    /// breadth-first order, predecessor edges remapped to the new
    /// generations), because the old forward executions are dead on this
    /// path: the cancelled/backed source already ended the flow through them.
    /// Only the re-entered task itself receives an `ExecuteTask` operation -
    /// its completion propagates into the recreated chain. `graph` is the
    /// snapshot's execution graph (rebuilt by the caller).
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
        // Forward closure of the re-entered source in breadth-first order
        // (edges always point forward, so BFS is a topological order): the
        // source and every execution downstream of it are dead on this path
        // and must be recreated as fresh generations.
        let mut order: Vec<ExecutionId> = Vec::new();
        let mut seen: HashMap<ExecutionId, ()> = HashMap::new();
        let mut queue = VecDeque::new();
        seen.insert(command.source.clone(), ());
        order.push(command.source.clone());
        queue.push_back(command.source.clone());
        while let Some(current) = queue.pop_front() {
            for successor in graph.successors(&current) {
                if seen.insert(successor.clone(), ()).is_some() {
                    continue;
                }
                order.push(successor.clone());
                queue.push_back(successor.clone());
            }
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
        // The re-entered source generation is admitted immediately (`Ready`,
        // carried by its `ExecuteTask` operation); the recreated downstream
        // starts `Created`, so ordinary propagation admits each of them in
        // turn as their predecessors settle.
        let next = remap.get(&command.source).cloned().ok_or_else(|| {
            GenerationError::TargetNeverInstantiated(command.source.task_id.clone())
        })?;
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
    use crate::kernel::{AggregateVersion, ClaimToken, ExecutionSnapshot};
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
        use crate::kernel::{ExecutionGraphBuilder, ExecutionNode};
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
            target_task_id: "target".into(),
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
}
