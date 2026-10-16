use crate::scheduler::core::{
    commit::process::ProcessSnapshot,
    model::{
        model_graph::NodeId,
        operation::{OperationId, OperationKind},
        task_graph::{TaskGraph, TaskId, TaskState},
        transition_plan::{
            OperationCommit, OperationIntent, OutboxIntent, StateChange, TaskCreate,
            TransitionPlan, VariableWrite,
        },
    },
};

use std::fmt;

/// Join semantics are explicit model policy, never inferred from timing or from
/// whichever predecessor happens to finish last.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum JoinPolicy {
    /// Every predecessor must have a successful terminal outcome.
    AllSucceeded,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PropagateCommand {
    pub(crate) process_id: String,
    pub(crate) task: TaskId,
    pub(crate) source_operation: OperationCommit,
    pub(crate) join_policy: JoinPolicy,
    /// The flow the host read from the declaring model, when the settling
    /// execution is a *step*: the node the flow goes to and whether it must be
    /// re-created first because its newest generation is already terminal.
    ///
    /// The graph's successors only encode the chains the seed and the
    /// container fan-outs wired (a declaration chain per container), while a
    /// step's flow is a *model* fact: its explicit `next` may jump outside the
    /// graph (backward to an ancestor, forward over a sibling), and a `while`
    /// step loops back into itself. `None` for a node the model does not name
    /// (a branch, a handler container, the workflow root), whose flow is the
    /// graph's own edges.
    pub(crate) flow: Option<FlowTarget>,
}

/// Where a step's flow goes once it settled — see [`PropagateCommand::flow`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FlowTarget {
    /// The model node the flow enters.
    pub(crate) node: String,
    /// Its newest generation is already terminal: the flow re-enters the node,
    /// so a fresh generation is created (a backward `next`, a `while` turn).
    pub(crate) reopen: bool,
    /// The node's durable auto decision (`__auto`) — what a re-entered
    /// generation's scope must carry, like every fan-out child does, or the
    /// engine would treat a waiting act as a self-completing one.
    pub(crate) auto: bool,
    /// The node's declared model variables (JSON text), seeded on the
    /// re-entered generation's own scope — the same fact the fan-out seeds on
    /// a child (`child_variables`), which the seed only wrote for generation 0.
    pub(crate) vars: Vec<(String, String)>,
}

/// The private variable an execution carries when its own `if`/`while` guard
/// decided it `Skipped`: the guard *fell through* — the legacy step skipped by
/// its condition, whose flow goes on to the next declared step — as opposed to
/// a skip the deadness cascade flowed in. A `__` fact: out of the expression
/// space and the read model, durable so a replay reads the same verdict.
pub(crate) const GUARD_SKIP_VAR: &str = "__guard_skip";

/// Whether one execution was skipped by its own guard (see [`GUARD_SKIP_VAR`]).
pub(crate) fn is_guard_skip(snapshot: &ProcessSnapshot, task: &TaskId) -> bool {
    snapshot
        .variables
        .contains_key(&(task.clone(), GUARD_SKIP_VAR.to_string()))
}

pub(crate) struct PropagationPlanner;

/// The terminal states a join accepts as "this predecessor is done and the
/// flow may continue". `Skipped` is deliberately absent: a skipped
/// predecessor means the path is dead (a cancellation cascade flowed through
/// it), so its successors are skipped too instead of running.
fn is_success(state: TaskState) -> bool {
    matches!(state, TaskState::Completed | TaskState::Submitted)
}

mod cascade;
mod errors;
mod join;
#[cfg(test)]
mod tests;

use self::join::*;

pub(crate) use errors::*;
