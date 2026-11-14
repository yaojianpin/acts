use crate::scheduler::core::{
    commit::aggregate::{ProcessSnapshot, TaskSnapshot},
    model::{
        operation::{OperationId, OperationKind},
        task_graph::{TaskGraph, TaskId, TaskState},
        transition_plan::{
            OperationCommit, OperationIntent, OutboxIntent, StateChange, TaskCreate,
            TransitionPlan, VariableWrite,
        },
    },
};

use crate::scheduler::core::plan::ActPlan;

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReenterCommand {
    pub(crate) process_id: String,
    /// The terminal execution requesting re-entry: a `Back`'s `Backed`
    /// source, or the finished (`Completed`/`Submitted`) act a `Cancel` was
    /// issued from.
    pub(crate) source: TaskId,
    /// Stable logical task identity to instantiate again.
    pub(crate) target_task_id: String,
    /// Whether the rewind is a `Cancel`: it closes the work ahead of the
    /// cancelled act (`Cancelled`), where a `Back` closes the branches beside
    /// its path (`Skipped`).
    pub(crate) cancels_forward: bool,
    /// The declared model variables per node, seeded on every recreated
    /// generation's own scope — the same fact the seed writes for generation 0
    /// and the fan-out seeds on a child. A rewind must not leave a re-run
    /// reading a scope the model declares it has.
    pub(crate) vars: HashMap<String, Vec<(String, String)>>,
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
        // A `Back` closes its source `Backed` and re-enters an earlier step.
        // A `Cancel` leaves the finished act it was issued from in the state
        // it already holds: that act is the origin of the rewind, and its
        // fresh generation re-runs it. Both are re-enterable; no other state
        // is.
        let reenterable = if command.cancels_forward {
            matches!(source.state, TaskState::Completed | TaskState::Submitted)
        } else {
            matches!(source.state, TaskState::Backed)
        };
        if !reenterable {
            return Err(GenerationError::SourceNotReenterable {
                source: command.source.clone(),
                state: source.state,
            });
        }
        let operation = &command.source_operation.id;
        if operation.process_id != command.process_id || operation.task != command.source {
            return Err(GenerationError::SourceOperationTargetsDifferentTask);
        }
        if command.target_task_id.trim().is_empty() {
            return Err(GenerationError::EmptyTargetTaskId);
        }
        // The closure roots at the re-entered task: a Cancel re-enters the
        // terminal source itself; a Back re-enters an EARLIER step named by
        // the client (`to`), so the root is that task's latest generation
        // and the source must lie inside the closure it spawns (the redo
        // re-runs everything downstream of the target, the source included).
        let root = if command.target_task_id == command.source.node.as_str() {
            command.source.clone()
        } else {
            snapshot
                .tasks
                .keys()
                .filter(|id| id.node.as_str() == command.target_task_id)
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
        // themselves through the identity map below). A node may appear
        // several times in the closure (a fan-out's parallel items share one
        // node id), so the allocation carries per node: the second
        // closed-over instance takes the generation after the first, and
        // recreating both as the same generation would refuse the plan.
        let mut allocated: HashMap<String, u64> = HashMap::new();
        let mut remap: HashMap<TaskId, TaskId> = HashMap::new();
        let mut closure: Vec<(TaskId, &TaskSnapshot, TaskId)> = Vec::with_capacity(order.len());
        for old_id in &order {
            let old = &snapshot.tasks[old_id];
            let generation = {
                let next = allocated
                    .entry(old_id.node.as_str().to_string())
                    .or_insert_with(|| {
                        snapshot
                            .tasks
                            .keys()
                            .filter(|id| id.node == old_id.node)
                            .map(|id| id.generation)
                            .max()
                            .unwrap_or(0)
                    });
                *next = next
                    .checked_add(1)
                    .ok_or(GenerationError::GenerationOverflow)?;
                *next
            };
            let new_id = TaskId::new(old_id.node.as_str(), generation)
                .map_err(|_| GenerationError::EmptyTargetTaskId)?;
            remap.insert(old_id.clone(), new_id.clone());
            closure.push((old_id.clone(), old, new_id));
        }
        // What the rewind settles depends on its direction.
        //
        // A `Back` walks from the source up to the re-entered step: the
        // containers on the path below the workflow root close `Backed`, and
        // every other superseded execution — the sibling branches beside the
        // path — closes `Skipped`.
        //
        // A `Cancel` marks the work *ahead* of the finished act it was issued
        // from: that act's own state stands (it is the origin, re-entered as
        // a new generation), the nearest forward tasks the flow admitted
        // close `Cancelled`, and everything under them — their subtrees and
        // the deeper seeded chain — closes `Skipped`. The cancelled front is
        // the direct successor set of the source; a successor the flow only
        // seeded (`Created`) never started, so it is skipped rather than
        // cancelled.
        let source_task = &snapshot.tasks[&command.source];
        let cancel_front: HashSet<TaskId> = if command.cancels_forward {
            graph.successors(&command.source).iter().cloned().collect()
        } else {
            HashSet::new()
        };
        let mut ancestors: Vec<TaskId> = Vec::new();
        let mut cursor = source_task.parent.clone();
        while let Some(id) = cursor {
            let Some(task) = snapshot.tasks.get(&id) else {
                break;
            };
            // The workflow root (and anything above it) is not a task the
            // rewind closes.
            if task.parent.is_none() {
                break;
            }
            ancestors.push(id.clone());
            cursor = task.parent.clone();
        }
        let state_changes: Vec<StateChange> = closure
            .iter()
            .filter(|(old_id, old, _)| *old_id != command.source && !old.state.is_terminal())
            .map(|(old_id, old, _)| {
                let to = if command.cancels_forward {
                    if cancel_front.contains(old_id)
                        && matches!(
                            old.state,
                            TaskState::Ready | TaskState::Running | TaskState::Waiting
                        )
                    {
                        TaskState::Cancelled
                    } else {
                        TaskState::Skipped
                    }
                } else if ancestors.contains(old_id) {
                    // The containers the walk passes close `Backed`.
                    TaskState::Backed
                } else {
                    // A `Back` closes every other superseded execution — the
                    // sibling branches beside its path and the forward work
                    // the flow had seeded but never ran — as `Skipped`, so
                    // the joins above the rewind see them settled.
                    TaskState::Skipped
                };
                StateChange {
                    task: old_id.clone(),
                    expected_revision: old.revision,
                    from: old.state,
                    to,
                }
            })
            .collect();

        // The re-entered root generation is admitted immediately (`Ready`,
        // carried by its `ExecuteTask` operation); the recreated downstream
        // starts `Created`, so ordinary propagation admits each of them in
        // turn as their predecessors settle.
        let next = remap.get(&root).cloned().ok_or_else(|| {
            GenerationError::TargetNeverInstantiated(root.node.as_str().to_owned())
        })?;
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
        // variable. The declared model variables are seeded alongside it,
        // exactly like the seed writes them for generation 0 and the fan-out
        // writes them on a child: a re-entered generation is a NEW execution,
        // so it starts from the declaration, never from the previous
        // generation's mutations. State that must survive a rewind lives on a
        // scope that outlives the generation (a workflow-level variable), not
        // on the step.
        let mut variable_writes: Vec<VariableWrite> = Vec::with_capacity(closure.len());
        for (old_id, _, new_id) in &closure {
            variable_writes.push(VariableWrite {
                scope: new_id.clone(),
                name: "auto".to_string(),
                value: snapshot
                    .variables
                    .get(&((*old_id).clone(), "auto".to_string()))
                    .cloned()
                    .unwrap_or_else(|| "1".to_string()),
            });
            let mut written = HashSet::from(["auto".to_string()]);
            for (name, value) in command.vars.get(old_id.node.as_str()).into_iter().flatten() {
                if !written.insert(name.clone()) {
                    continue;
                }
                variable_writes.push(VariableWrite {
                    scope: new_id.clone(),
                    name: name.clone(),
                    value: value.clone(),
                });
            }
        }

        // The re-entered source generation is the one that runs again; the
        // recreated downstream waits for its propagation.
        let next_operation = OperationId {
            process_id: command.process_id.clone(),
            task: next.clone(),
            sequence: 0,
        };
        let plan = TransitionPlan {
            process_id: command.process_id.clone(),
            expected: snapshot.version.clone(),
            task_creates,
            state_changes,
            variable_writes,
            source_operation: Some(command.source_operation.clone()),
            operations: vec![OperationIntent {
                id: next_operation,
                kind: OperationKind::ExecuteTask,
                ready_at_ms: None,
            }],
            outbox: vec![OutboxIntent {
                key: format!(
                    "reenter:{}:{}:{}:{}",
                    command.process_id,
                    next.node.as_str(),
                    next.generation,
                    command.source_operation.id.sequence,
                ),
                topic: "task.reentered".into(),
                payload: format!("{}:{}", next.node.as_str(), next.generation),
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
