//! The pure part of a timed-out execution's branch dispatch.
//!
//! The legacy engine fired a step's `timeouts` from the process tick: while the
//! step was `Running`/`Interrupt`, `Step::on_timeout` froze the task's cost into
//! its data (the `__cost` task datum `$cost()`/`$cost_in(...)` read) and
//! dispatched the first declared branch that had neither fired (the one-shot
//! `__timeouts` marker) nor was in flight. A branch whose guard did not hold
//! skipped *without* claiming, so a later tick re-evaluated it — that is what
//! opens the second window of a `timeouts` list.
//!
//! The kernel keeps the same three facts in the aggregate — the cost start
//! (`__started`), the frozen cost (`__cost`) and the one-shot marker
//! (`__timeouts`) — and this module is the pure part of the mechanism: which
//! branch the next tick may dispatch, what its cost is, and how the marker
//! advances. The host supplies `now` and the evaluated guard; nothing here
//! reads a clock, a spec or a store.

use crate::{
    scheduler::core::{
        commit::process::ProcessSnapshot,
        model::{
            model_graph::NodeId,
            task_graph::TaskId,
            transition_plan::{OperationCommit, VariableWrite},
        },
        plan::container::{
            ContainerChild, ContainerChildPlan, ContainerFanoutCommand, ContainerPredecessor,
        },
    },
    utils::consts,
};

mod dispatch;
mod marker;
#[cfg(test)]
mod tests;

pub(crate) use dispatch::*;
pub(crate) use marker::*;
