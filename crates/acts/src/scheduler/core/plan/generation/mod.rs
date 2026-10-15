use crate::scheduler::core::{
    commit::process::{ExecutionSnapshot, ProcessSnapshot},
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
        snapshot: &ProcessSnapshot,
        command: &ReenterCommand,
    ) -> Result<TransitionPlan, GenerationError> {
        if snapshot.process_id != command.process_id {
            return Err(GenerationError::WrongProcess);
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
                receipt: None,
            }],
        };
        plan.validate()
            .map_err(|error| GenerationError::InvalidPlan(error.to_string()))?;
        Ok(plan)
    }
}

mod errors;
#[cfg(test)]
mod tests;

pub(crate) use errors::*;
