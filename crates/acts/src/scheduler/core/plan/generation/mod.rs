use crate::scheduler::core::{
    commit::aggregate::{ProcessSnapshot, TaskSnapshot},
    model::{
        operation::{OperationId, OperationKind},
        task_graph::{TaskGraph, TaskId, TaskState},
        transition_plan::{
            OperationCommit, OperationIntent, OutboxIntent, TaskCreate, TransitionPlan,
            VariableWrite,
        },
    },
};

use crate::scheduler::core::plan::ActPlan;

use std::collections::{HashMap, VecDeque};
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReenterCommand {
    pub(crate) process_id: String,
    /// The terminal execution requesting re-entry, normally a Backed task.
    pub(crate) source: TaskId,
    /// Stable logical task identity to instantiate again.
    pub(crate) target_task_id: String,
    pub(crate) source_operation: OperationCommit,
}

pub(crate) struct Reenter;

impl ActPlan for Reenter {
    type Reading<'a> = (&'a TaskGraph, &'a ProcessSnapshot);
    type Command<'a> = ReenterCommand;
    type Output = TransitionPlan;
    type Error = GenerationError;

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
    fn plan(
        reading: Self::Reading<'_>,
        command: &Self::Command<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let (graph, snapshot) = reading;
        if snapshot.process_id != command.process_id {
            return Err(GenerationError::WrongProcess);
        }
        let source = snapshot
            .tasks
            .get(&command.source)
            .ok_or_else(|| GenerationError::TaskNotFound(command.source.clone()))?;
        if !matches!(source.state, TaskState::Backed | TaskState::Cancelled) {
            return Err(GenerationError::SourceNotReenterable {
                source: command.source.clone(),
                state: source.state,
            });
        }
        let operation = &command.source_operation.id;
        if operation.process_id != command.process_id
            || operation.node != command.source.node
            || operation.generation != command.source.generation
        {
            return Err(GenerationError::SourceOperationTargetsDifferentTask);
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
        let root = if command.target_task_id == command.source.node {
            command.source.clone()
        } else {
            snapshot
                .tasks
                .keys()
                .filter(|id| id.node == command.target_task_id)
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
        let mut order: Vec<TaskId> = Vec::new();
        let mut seen: HashMap<TaskId, ()> = HashMap::new();
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
        let mut remap: HashMap<TaskId, TaskId> = HashMap::new();
        let mut closure: Vec<(TaskId, &TaskSnapshot, TaskId)> = Vec::with_capacity(order.len());
        for old_id in &order {
            let old = &snapshot.tasks[old_id];
            let generation = snapshot
                .tasks
                .keys()
                .filter(|id| id.node == old_id.node)
                .map(|id| id.generation)
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or(GenerationError::GenerationOverflow)?;
            let new_id = TaskId::new(old_id.node.clone(), generation)
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
            .ok_or_else(|| GenerationError::TargetNeverInstantiated(root.node.clone()))?;
        let task_creates: Vec<TaskCreate> = closure
            .iter()
            .map(|(old_id, old, new_id)| TaskCreate {
                task: new_id.clone(),
                model_node: old.model_node.clone(),
                // A nested closure's children belong to the re-entered
                // generation, not the dead one: the parent edge remaps like
                // the predecessor edges (outside the closure - the root -
                // the old parent stays).
                parent: old
                    .parent
                    .as_ref()
                    .map(|parent| remap.get(parent).cloned().unwrap_or_else(|| parent.clone())),
                predecessors: snapshot.tasks[old_id]
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
                    TaskState::Ready
                } else {
                    TaskState::Created
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
            node: next.node.clone(),
            generation: next.generation,
            sequence: 0,
        };
        let plan = TransitionPlan {
            process_id: command.process_id.clone(),
            expected: snapshot.version.clone(),
            task_creates,
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
                    next.node,
                    next.generation,
                    command.source_operation.id.sequence,
                ),
                topic: "task.reentered".into(),
                payload: format!("{}:{}", next.node, next.generation),
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
