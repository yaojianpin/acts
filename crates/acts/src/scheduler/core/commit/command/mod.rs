//! The client-action planner: which durable plan one `proc:*`/`task:*` decision
//! turns into (its action vocabulary, the target state it may move to, and the
//! input validation both share).
use crate::scheduler::core::{
    commit::aggregate::ProcessSnapshot,
    model::{
        operation::{OperationId, OperationKind},
        task_graph::{TaskId, TaskState},
        transition_plan::{
            OperationCommit, OperationIntent, OutboxIntent, StateChange, TransitionPlan,
            VariableWrite,
        },
    },
};

use std::collections::HashSet;
use std::fmt;

mod action;
mod errors;
mod planner;
mod target;
#[cfg(test)]
mod tests;

pub(crate) use action::*;
pub(crate) use errors::*;
pub(crate) use planner::*;
pub(crate) use target::*;
