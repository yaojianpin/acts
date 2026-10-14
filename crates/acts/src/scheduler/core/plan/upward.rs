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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum UpwardError {
    WrongAggregate,
    ExecutionNotFound(ExecutionId),
    UnsupportedTarget(ExecutionState),
    SourceNotDecided {
        source: ExecutionId,
        state: ExecutionState,
        to: ExecutionState,
    },
}
impl fmt::Display for UpwardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "upward walk error: {self:?}")
    }
}
impl std::error::Error for UpwardError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::core::commit::aggregate::ExecutionSnapshot;
    use crate::scheduler::core::model::model_graph::NodeId;
    use crate::scheduler::core::model::transition_plan::AggregateVersion;
    use std::collections::{BTreeMap, HashMap};

    fn id(task: &str, generation: u64) -> ExecutionId {
        ExecutionId::new(task, generation).unwrap()
    }
    fn execution(
        task: &str,
        generation: u64,
        parent: Option<ExecutionId>,
        state: ExecutionState,
    ) -> (ExecutionId, ExecutionSnapshot) {
        (
            id(task, generation),
            ExecutionSnapshot {
                model_node: NodeId::parse(task).unwrap(),
                parent,
                predecessors: vec![],
                state,
                revision: 3,
            },
        )
    }
    fn snapshot(executions: Vec<(ExecutionId, ExecutionSnapshot)>) -> AggregateSnapshot {
        AggregateSnapshot {
            process_id: "p1".into(),
            version: AggregateVersion {
                graph: 2,
                process: 5,
                operations: 3,
            },
            executions: HashMap::from_iter(executions),
            variables: BTreeMap::new(),
            operations: HashMap::new(),
            outbox: HashMap::new(),
        }
    }
    fn abort_command(source: ExecutionId) -> UpwardCommand {
        UpwardCommand {
            process_id: "p1".into(),
            source,
            to: ExecutionState::Aborted,
        }
    }

    #[test]
    fn upward_walk_aborts_every_ancestor_and_seeds_their_cascade() {
        let snapshot = snapshot(vec![
            execution("$", 0, None, ExecutionState::Completed),
            execution("step1", 0, Some(id("$", 0)), ExecutionState::Completed),
            execution("step2", 0, Some(id("step1", 0)), ExecutionState::Aborted),
        ]);
        let (changes, operations) =
            UpwardPlanner::plan_upward(&snapshot, &abort_command(id("step2", 0))).unwrap();
        // Nearest ancestor first, root last — the legacy walk's order.
        let executed: Vec<&str> = changes
            .iter()
            .map(|change| change.execution.task_id.as_str())
            .collect();
        assert_eq!(executed, vec!["step1", "$"]);
        assert!(
            changes
                .iter()
                .all(|change| change.to == ExecutionState::Aborted)
        );
        assert_eq!(changes[0].from, ExecutionState::Completed);
        assert_eq!(operations.len(), 2);
        assert!(
            operations
                .iter()
                .all(|op| op.kind == OperationKind::PropagateTask)
        );
        assert_eq!(operations[0].id.task_id, "step1");
        assert_eq!(operations[1].id.task_id, "$");
    }

    #[test]
    fn an_already_aborted_ancestor_is_left_to_its_own_cascade() {
        let snapshot = snapshot(vec![
            execution("$", 0, None, ExecutionState::Aborted),
            execution("step1", 0, Some(id("$", 0)), ExecutionState::Aborted),
            execution("step2", 0, Some(id("step1", 0)), ExecutionState::Aborted),
        ]);
        let (changes, operations) =
            UpwardPlanner::plan_upward(&snapshot, &abort_command(id("step2", 0))).unwrap();
        assert!(changes.is_empty());
        assert!(operations.is_empty());
    }

    #[test]
    fn the_walk_rides_an_aborted_source_only() {
        let snapshot = snapshot(vec![execution("step1", 0, None, ExecutionState::Cancelled)]);
        let error =
            UpwardPlanner::plan_upward(&snapshot, &abort_command(id("step1", 0))).unwrap_err();
        assert!(matches!(error, UpwardError::SourceNotDecided { .. }));
    }

    #[test]
    fn the_error_walk_rides_a_failed_source_upward() {
        let snapshot = snapshot(vec![
            execution("$", 0, None, ExecutionState::Completed),
            execution("step1", 0, Some(id("$", 0)), ExecutionState::Failed),
        ]);
        let command = UpwardCommand {
            process_id: "p1".into(),
            source: id("step1", 0),
            to: ExecutionState::Failed,
        };
        let (changes, operations) = UpwardPlanner::plan_upward(&snapshot, &command).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].execution, id("$", 0));
        assert_eq!(changes[0].to, ExecutionState::Failed);
        assert_eq!(operations.len(), 1);
        assert_eq!(operations[0].id.task_id, "$");
    }

    #[test]
    fn only_abort_and_fail_walk_upward() {
        let snapshot = snapshot(vec![execution("step1", 0, None, ExecutionState::Skipped)]);
        let error = UpwardPlanner::plan_upward(
            &snapshot,
            &UpwardCommand {
                process_id: "p1".into(),
                source: id("step1", 0),
                to: ExecutionState::Skipped,
            },
        )
        .unwrap_err();
        assert!(matches!(error, UpwardError::UnsupportedTarget(_)));
    }
}
