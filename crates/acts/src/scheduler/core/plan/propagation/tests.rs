use super::*;

use crate::scheduler::core::commit::aggregate::TaskSnapshot;
use crate::scheduler::core::model::model_graph::NodeId;
use crate::scheduler::core::model::operation::ClaimToken;
use crate::scheduler::core::model::task_graph::TaskGraphBuilder;
use crate::scheduler::core::model::task_graph::TaskNode;
use crate::scheduler::core::model::transition_plan::ProcessVersion;
use crate::scheduler::core::plan::ActPlan;
use std::collections::{BTreeMap, HashMap};

fn id(name: &str) -> TaskId {
    TaskId::new(name, 0).unwrap()
}
fn node(name: &str) -> TaskNode {
    TaskNode {
        id: id(name),
        model_node: NodeId::parse(name).unwrap(),
        state: TaskState::Created,
        revision: 0,
    }
}
fn graph() -> TaskGraph {
    let mut b = TaskGraphBuilder::new("p1", 4).unwrap();
    for name in ["root", "a", "b", "join"] {
        b.add_node(node(name)).unwrap();
    }
    b.set_root(id("root")).unwrap();
    for name in ["a", "b", "join"] {
        b.add_child(id("root"), id(name)).unwrap();
    }
    b.add_successor(id("a"), id("join")).unwrap();
    b.add_successor(id("b"), id("join")).unwrap();
    b.build().unwrap()
}
fn snapshot(a: TaskState, b: TaskState) -> ProcessSnapshot {
    ProcessSnapshot {
        process_id: "p1".into(),
        version: ProcessVersion {
            graph: 4,
            process: 4,
            operations: 2,
        },
        tasks: HashMap::from([
            (
                id("root"),
                TaskSnapshot {
                    model_node: NodeId::parse("node").unwrap(),
                    parent: None,
                    predecessors: vec![],
                    state: TaskState::Running,
                    revision: 0,
                },
            ),
            (
                id("a"),
                TaskSnapshot {
                    model_node: NodeId::parse("node").unwrap(),
                    parent: None,
                    predecessors: vec![],
                    state: a,
                    revision: 1,
                },
            ),
            (
                id("b"),
                TaskSnapshot {
                    model_node: NodeId::parse("node").unwrap(),
                    parent: None,
                    predecessors: vec![],
                    state: b,
                    revision: 1,
                },
            ),
            (
                id("join"),
                TaskSnapshot {
                    model_node: NodeId::parse("node").unwrap(),
                    parent: None,
                    predecessors: vec![],
                    state: TaskState::Created,
                    revision: 0,
                },
            ),
        ]),
        variables: BTreeMap::new(),
        operations: HashMap::new(),
        outbox: HashMap::new(),
    }
}
fn command(source: &str, policy: JoinPolicy) -> PropagateCommand {
    PropagateCommand {
        process_id: "p1".into(),
        task: id(source),
        join_policy: policy,
        source_operation: OperationCommit {
            id: OperationId {
                process_id: "p1".into(),
                node: source.into(),
                generation: 0,
                sequence: 1,
            },
            claim: ClaimToken {
                owner: "r1".into(),
                fence: 2,
                lease_until_ms: i64::MAX,
                heartbeat_revision: 0,
            },
        },
        flow: None,
    }
}

#[test]
fn join_waits_until_every_predecessor_is_terminal() {
    let plan = Propagate::plan(
        (
            &graph(),
            &snapshot(TaskState::Completed, TaskState::Running),
        ),
        &command("a", JoinPolicy::AllSucceeded),
    )
    .unwrap();
    assert!(plan.state_changes.is_empty());
    assert!(plan.operations.is_empty());
}

#[test]
fn last_successful_predecessor_schedules_join_once() {
    let plan = Propagate::plan(
        (
            &graph(),
            &snapshot(TaskState::Completed, TaskState::Completed),
        ),
        &command("b", JoinPolicy::AllSucceeded),
    )
    .unwrap();
    assert_eq!(plan.state_changes.len(), 1);
    assert_eq!(plan.state_changes[0].task, id("join"));
    assert_eq!(plan.operations[0].kind, OperationKind::ExecuteTask);
}

#[test]
fn dead_predecessor_puts_success_join_into_the_skip_cascade() {
    let plan = Propagate::plan(
        (&graph(), &snapshot(TaskState::Failed, TaskState::Completed)),
        &command("b", JoinPolicy::AllSucceeded),
    )
    .unwrap();
    // The join is admitted and handed a skip cascade: its one operation
    // is a SkipTask, not an execution.
    assert_eq!(plan.state_changes.len(), 1);
    assert_eq!(plan.state_changes[0].task, id("join"));
    assert_eq!(plan.state_changes[0].to, TaskState::Ready);
    assert_eq!(plan.operations.len(), 1);
    assert_eq!(plan.operations[0].kind, OperationKind::SkipTask);
    assert_eq!(plan.operations[0].id.node, "join");
}

#[test]
fn dead_source_sends_admitted_unfinished_successors_into_the_cascade() {
    // `join` is already `Ready` (admitted, start hop pending) when its
    // predecessor is cancelled: it must skip, not run behind a dead path.
    let mut snap = snapshot(TaskState::Cancelled, TaskState::Completed);
    if let Some(target) = snap.tasks.get_mut(&id("join")) {
        target.state = TaskState::Ready;
    }
    let plan = Propagate::plan((&graph(), &snap), &command("a", JoinPolicy::AllSucceeded)).unwrap();
    // A `Ready` target needs no admission change, just the skip operation.
    assert!(plan.state_changes.is_empty());
    assert_eq!(plan.operations.len(), 1);
    assert_eq!(plan.operations[0].kind, OperationKind::SkipTask);
    assert_eq!(plan.operations[0].id.node, "join");
}

#[test]
fn a_running_successor_of_a_dead_source_is_skipped_in_place() {
    let mut b = TaskGraphBuilder::new("p1", 4).unwrap();
    for name in ["root", "a", "b"] {
        b.add_node(TaskNode {
            id: id(name),
            model_node: NodeId::parse(name).unwrap(),
            state: TaskState::Created,
            revision: 0,
        })
        .unwrap();
    }
    b.set_root(id("root")).unwrap();
    b.add_child(id("root"), id("a")).unwrap();
    b.add_child(id("root"), id("b")).unwrap();
    b.add_successor(id("a"), id("b")).unwrap();
    let graph = b.build().unwrap();

    let mut snapshot = snapshot(TaskState::Cancelled, TaskState::Running);
    if let Some(target) = snapshot.tasks.get_mut(&id("b")) {
        target.revision = 6;
    }
    let plan =
        Propagate::plan((&graph, &snapshot), &command("a", JoinPolicy::AllSucceeded)).unwrap();
    // The plan only schedules the skip; the hop owns the state change
    // against the authoritative state.
    assert!(plan.state_changes.is_empty());
    assert_eq!(plan.operations.len(), 1);
    assert_eq!(plan.operations[0].kind, OperationKind::SkipTask);
    assert_eq!(plan.operations[0].id.node, "b");
    assert_eq!(plan.operations[0].id.sequence, 0, "no prior hops on b");
}

#[test]
fn a_live_source_leaves_admitted_successors_alone() {
    let plan = Propagate::plan(
        (&graph(), &snapshot(TaskState::Completed, TaskState::Ready)),
        &command("a", JoinPolicy::AllSucceeded),
    )
    .unwrap();
    assert!(plan.state_changes.is_empty());
    assert!(plan.operations.is_empty());
}
