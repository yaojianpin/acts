//! The decision walks' upward half.
//!
//! Two legacy walks rewrite the ancestor chain of a decided execution, and
//! both do it unconditionally — terminal ancestors included:
//!
//! - the abort walk terminates the run from the acting act up through its
//!   ancestors, every one of them `Aborted`;
//! - the error walk (`emit_error` → `parent.set_err`) decides every
//!   ancestor `Error` when nothing catches.
//!
//! The downward half of either death is the ordinary dead-source cascade
//! ([`crate::scheduler::core::plan::propagation::Propagate`]); this planner produces the upward half:
//! one atomic batch of ancestor state changes plus the `PropagateTask`
//! operations whose operations spread the death into the remaining subtrees.

use crate::scheduler::core::{
    commit::aggregate::ProcessSnapshot,
    model::{
        operation::OperationKind,
        task_graph::{TaskId, TaskState},
        transition_plan::{OperationIntent, StateChange},
    },
};

use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UpwardCommand {
    pub(crate) process_id: String,
    /// The decided execution whose ancestors the walk rewrites; its state
    /// must already be the walk's target state.
    pub(crate) source: TaskId,
    /// The decision spreading upward: `Aborted` (the abort walk) or
    /// `Failed` (the error walk).
    pub(crate) to: TaskState,
}

pub(crate) struct Upward;

#[allow(clippy::type_complexity)]
impl crate::scheduler::core::plan::ActPlan for Upward {
    type Reading<'a> = &'a ProcessSnapshot;
    type Command<'a> = UpwardCommand;
    type Output = (Vec<StateChange>, Vec<OperationIntent>);
    type Error = UpwardError;

    fn plan(
        reading: Self::Reading<'_>,
        command: &Self::Command<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let snapshot = reading;
        if snapshot.process_id != command.process_id {
            return Err(UpwardError::WrongProcess);
        }
        if !matches!(command.to, TaskState::Aborted | TaskState::Failed) {
            return Err(UpwardError::UnsupportedTarget(command.to));
        }
        let source = snapshot
            .tasks
            .get(&command.source)
            .ok_or_else(|| UpwardError::TaskNotFound(command.source.clone()))?;
        if source.state != command.to {
            return Err(UpwardError::SourceNotDecided {
                source: command.source.clone(),
                state: source.state,
                to: command.to,
            });
        }

        let mut state_changes = Vec::new();
        let mut operations = Vec::new();
        let mut cursor = source.parent.clone();
        while let Some(ancestor) = cursor {
            let task = snapshot
                .tasks
                .get(&ancestor)
                .ok_or_else(|| UpwardError::TaskNotFound(ancestor.clone()))?;
            if task.state != command.to {
                // The legacy walk decides every ancestor unconditionally,
                // rewriting terminal ones; only an ancestor already carrying
                // the decision (a racing second walk) keeps its state — its
                // own cascade ran with the first decision.
                state_changes.push(StateChange {
                    task: ancestor.clone(),
                    expected_revision: task.revision,
                    from: task.state,
                    to: command.to,
                });
                operations.push(OperationIntent {
                    id: crate::scheduler::core::model::operation::OperationId {
                        process_id: command.process_id.clone(),
                        task: ancestor.clone(),
                        sequence: snapshot
                            .next_operation_sequence(ancestor.node.as_str(), ancestor.generation),
                    },
                    kind: OperationKind::PropagateTask,
                    ready_at_ms: None,
                });
            }
            cursor = task.parent.clone();
        }
        Ok((state_changes, operations))
    }
}

mod errors;
#[cfg(test)]
mod tests;

pub(crate) use errors::*;
