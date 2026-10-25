use super::*;

use crate::scheduler::core::commit::aggregate::TaskSnapshot;
use crate::scheduler::core::model::model_graph::NodeId;
use crate::scheduler::core::model::transition_plan::ProcessVersion;
use crate::scheduler::core::plan::ActPlan;
use std::collections::{BTreeMap, HashMap};

fn id(task: &str, generation: u64) -> TaskId {
    TaskId::new(task, generation).unwrap()
}
fn task(
    task: &str,
    generation: u64,
    parent: Option<TaskId>,
    state: TaskState,
) -> (TaskId, TaskSnapshot) {
    (
        id(task, generation),
        TaskSnapshot {
            model_node: NodeId::parse(task).unwrap(),
            parent,
            predecessors: vec![],
            state,
            revision: 3,
        },
    )
}
fn snapshot(tasks: Vec<(TaskId, TaskSnapshot)>) -> ProcessSnapshot {
    ProcessSnapshot {
        process_id: "p1".into(),
        version: ProcessVersion {
            graph: 2,
            process: 5,
            operations: 3,
        },
        tasks: HashMap::from_iter(tasks),
        variables: BTreeMap::new(),
        operations: HashMap::new(),
        outbox: HashMap::new(),
    }
}
fn abort_command(source: TaskId) -> UpwardCommand {
    UpwardCommand {
        process_id: "p1".into(),
        source,
        to: TaskState::Aborted,
        stop: None,
    }
}

#[test]
fn upward_walk_aborts_every_ancestor_and_seeds_their_cascade() {
    let snapshot = snapshot(vec![
        task("$", 0, None, TaskState::Completed),
        task("step1", 0, Some(id("$", 0)), TaskState::Completed),
        task("step2", 0, Some(id("step1", 0)), TaskState::Aborted),
    ]);
    let (changes, operations) = Upward::plan(&snapshot, &abort_command(id("step2", 0))).unwrap();
    // Nearest ancestor first, root last — the legacy walk's order.
    let executed: Vec<&str> = changes
        .iter()
        .map(|change| change.task.node.as_str())
        .collect();
    assert_eq!(executed, vec!["step1", "$"]);
    assert!(changes.iter().all(|change| change.to == TaskState::Aborted));
    assert_eq!(changes[0].from, TaskState::Completed);
    assert_eq!(operations.len(), 2);
    assert!(
        operations
            .iter()
            .all(|op| op.kind == OperationKind::PropagateTask)
    );
    assert_eq!(operations[0].id.task.node.as_str(), "step1");
    assert_eq!(operations[1].id.task.node.as_str(), "$");
}

#[test]
fn an_already_aborted_ancestor_is_left_to_its_own_cascade() {
    let snapshot = snapshot(vec![
        task("$", 0, None, TaskState::Aborted),
        task("step1", 0, Some(id("$", 0)), TaskState::Aborted),
        task("step2", 0, Some(id("step1", 0)), TaskState::Aborted),
    ]);
    let (changes, operations) = Upward::plan(&snapshot, &abort_command(id("step2", 0))).unwrap();
    assert!(changes.is_empty());
    assert!(operations.is_empty());
}

#[test]
fn the_walk_rides_an_aborted_source_only() {
    let snapshot = snapshot(vec![task("step1", 0, None, TaskState::Cancelled)]);
    let error = Upward::plan(&snapshot, &abort_command(id("step1", 0))).unwrap_err();
    assert!(matches!(error, UpwardError::SourceNotDecided { .. }));
}

#[test]
fn the_error_walk_rides_a_failed_source_upward() {
    let snapshot = snapshot(vec![
        task("$", 0, None, TaskState::Completed),
        task("step1", 0, Some(id("$", 0)), TaskState::Failed),
    ]);
    let command = UpwardCommand {
        process_id: "p1".into(),
        source: id("step1", 0),
        to: TaskState::Failed,
        stop: None,
    };
    let (changes, operations) = Upward::plan(&snapshot, &command).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].task, id("$", 0));
    assert_eq!(changes[0].to, TaskState::Failed);
    assert_eq!(operations.len(), 1);
    assert_eq!(operations[0].id.task.node.as_str(), "$");
}

#[test]
fn only_abort_and_fail_walk_upward() {
    let snapshot = snapshot(vec![task("step1", 0, None, TaskState::Skipped)]);
    let error = Upward::plan(
        &snapshot,
        &UpwardCommand {
            process_id: "p1".into(),
            source: id("step1", 0),
            to: TaskState::Skipped,
            stop: None,
        },
    )
    .unwrap_err();
    assert!(matches!(error, UpwardError::UnsupportedTarget(_)));
}

#[test]
fn the_command_must_address_the_snapshot_process() {
    let snapshot = snapshot(vec![task("step1", 0, None, TaskState::Aborted)]);
    let error = Upward::plan(
        &snapshot,
        &UpwardCommand {
            process_id: "other".into(),
            source: id("step1", 0),
            to: TaskState::Aborted,
            stop: None,
        },
    )
    .unwrap_err();
    assert_eq!(error, UpwardError::WrongProcess);
}

#[test]
fn a_missing_source_or_ancestor_is_named() {
    let base = snapshot(vec![task("step1", 0, None, TaskState::Aborted)]);
    assert!(matches!(
        Upward::plan(&base, &abort_command(id("ghost", 0))),
        Err(UpwardError::TaskNotFound(_))
    ));
    // An ancestor edge that names a missing task is corrupt.
    let mut corrupt = base;
    corrupt.tasks.get_mut(&id("step1", 0)).unwrap().parent = Some(id("ghost", 0));
    assert!(matches!(
        Upward::plan(&corrupt, &abort_command(id("step1", 0))),
        Err(UpwardError::TaskNotFound(_))
    ));
}

#[test]
fn upward_errors_render() {
    for error in [
        UpwardError::WrongProcess,
        UpwardError::TaskNotFound(id("a", 0)),
        UpwardError::UnsupportedTarget(TaskState::Skipped),
        UpwardError::SourceNotDecided {
            source: id("a", 0),
            state: TaskState::Running,
            to: TaskState::Aborted,
        },
    ] {
        assert!(error.to_string().starts_with("upward walk error:"));
    }
}
