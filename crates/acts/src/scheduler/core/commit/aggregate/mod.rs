//! The aggregate document of one process: the snapshot the commit services mutate,
//! the repository boundary that stores it, and the canonical bytes the commit digest is
//! taken over.
use crate::scheduler::core::{
    model::{
        model_graph::NodeId,
        operation::{CommitToken, OperationEvent, OperationRecord, OperationState},
        task_graph::{TaskId, TaskState},
        transition_plan::{ProcessVersion, TransitionPlan},
    },
    outbox::records::{OutboxId, OutboxRecord},
};

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

mod canonical;
mod commit;
mod errors;
mod repository;
mod snapshot;
#[cfg(test)]
mod tests;

pub(crate) use canonical::*;
pub(crate) use commit::*;
pub(crate) use errors::*;
pub(crate) use repository::*;
pub(crate) use snapshot::*;
