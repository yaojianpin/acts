//! Container fan-out and join planning.
//!
//! A *container* execution holds model children: a step with `branches`, a
//! step with `catches`, a branch with its own `steps`, or the workflow root
//! over its seeded chain. Instead of completing at its effect point, a
//! container fans out — every child becomes an execution in one atomic plan —
//! and stays `Running` (in the `children` flow, see [`FLOW_VAR`]) until all of
//! its children are terminal (the join). That is the legacy engine's shape: a
//! step stays running while `check_in_children`'s children run and
//! `auto_complete` finishes it once they all settled.
//!
//! `Running` is the park state, not `Waiting`: `Waiting` is the legacy
//! `Interrupt` — an execution whose own act nobody answered yet — and a child's
//! settlement must never decide one of those.
//!
//! A failed step fans its `catches` out through the same plan but parks in
//! `Running`, and its join is the *catch* join ([`ContainerPlanner::plan_catch_join`]):
//! the legacy `Step::on_error` keeps the failed step running while the CATCH
//! children decide whether the error was caught, and `auto_complete` decides
//! it from their states.
//!
//! The planner is pure: it receives the snapshot, the already-evaluated
//! selection (branch predicates are the host's job, selection semantics are
//! [`crate::scheduler::core::plan::branch::BranchPlanner`]'s), and the per-child durable facts (`auto`,
//! declared vars). Admission of each child goes through
//! [`crate::scheduler::core::plan::task_admission::GenerationAdmissionPolicy`], so a re-entered container reuses the
//! generations a Cancel rewind already recreated instead of allocating
//! duplicates.

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
    plan::task_admission::{ExistingGeneration, SlotObservation},
};

use std::fmt;

pub(crate) struct ContainerPlanner;

mod catch_join;
mod child;
mod errors;
mod fanout;
mod flow;
mod join;
#[cfg(test)]
mod tests;

// The children's whole `pub(crate)` surface is re-exported by glob, so every
// path the rest of the crate (and the module's own tests) address as
// `plan::container::…` keeps resolving here — the module is the directory now.
pub(crate) use catch_join::*;
pub(crate) use child::*;
pub(crate) use errors::*;
pub(crate) use fanout::*;
pub(crate) use flow::*;
pub(crate) use join::*;
