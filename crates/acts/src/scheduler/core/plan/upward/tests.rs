use super::*;
use crate::scheduler::core::commit::process::ExecutionSnapshot;
use crate::scheduler::core::model::model_graph::NodeId;
use crate::scheduler::core::model::transition_plan::ProcessVersion;
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
fn snapshot(executions: Vec<(ExecutionId, ExecutionSnapshot)>) -> ProcessSnapshot {
    ProcessSnapshot {
        process_id: "p1".into(),
        version: ProcessVersion {
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
    let error = UpwardPlanner::plan_upward(&snapshot, &abort_command(id("step1", 0))).unwrap_err();
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
