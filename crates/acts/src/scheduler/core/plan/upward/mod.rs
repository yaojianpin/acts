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
//! ([`crate::scheduler::core::plan::propagation::PropagationPlanner`]); this planner produces the upward half:
//! one atomic batch of ancestor state changes plus the `PropagateTask`
//! operations whose hops spread the death into the remaining subtrees.

use crate::scheduler::core::{
    commit::aggregate::AggregateSnapshot,
    model::{
        execution_graph::{ExecutionId, ExecutionState},
        operation::OperationKind,
        transition_plan::{OperationIntent, StateChange},
    },
};

use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UpwardCommand {
    pub(crate) process_id: String,
    /// The decided execution whose ancestors the walk rewrites; its state
    /// must already be the walk's target state.
    pub(crate) source: ExecutionId,
    /// The decision spreading upward: `Aborted` (the abort walk) or
    /// `Failed` (the error walk).
    pub(crate) to: ExecutionState,
}

pub(crate) struct UpwardPlanner;

impl UpwardPlanner {
    #[allow(clippy::type_complexity)]
    pub(crate) fn plan_upward(
        snapshot: &AggregateSnapshot,
        command: &UpwardCommand,
    ) -> Result<(Vec<StateChange>, Vec<OperationIntent>), UpwardError> {
        if snapshot.process_id != command.process_id {
            return Err(UpwardError::WrongAggregate);
        }
        if !matches!(command.to, ExecutionState::Aborted | ExecutionState::Failed) {
            return Err(UpwardError::UnsupportedTarget(command.to));
        }
        let source = snapshot
            .executions
            .get(&command.source)
            .ok_or_else(|| UpwardError::ExecutionNotFound(command.source.clone()))?;
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
            let execution = snapshot
                .executions
                .get(&ancestor)
                .ok_or_else(|| UpwardError::ExecutionNotFound(ancestor.clone()))?;
            if execution.state != command.to {
                // The legacy walk decides every ancestor unconditionally,
                // rewriting terminal ones; only an ancestor already carrying
                // the decision (a racing second walk) keeps its state — its
                // own cascade ran with the first decision.
                state_changes.push(StateChange {
                    execution: ancestor.clone(),
                    expected_revision: execution.revision,
                    from: execution.state,
                    to: command.to,
                });
                operations.push(OperationIntent {
                    id: crate::scheduler::core::model::operation::OperationId {
                        process_id: command.process_id.clone(),
                        task_id: ancestor.task_id.clone(),
                        generation: ancestor.generation,
                        sequence: snapshot
                            .next_operation_sequence(&ancestor.task_id, ancestor.generation),
                    },
                    kind: OperationKind::PropagateTask,
                });
            }
            cursor = execution.parent.clone();
        }
        Ok((state_changes, operations))
    }
}

mod errors;
#[cfg(test)]
mod tests;

pub(crate) use errors::*;
