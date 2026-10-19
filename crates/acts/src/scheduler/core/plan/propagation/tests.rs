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
                task: id(source),
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
    assert_eq!(plan.operations[0].id.task.node.as_str(), "join");
}

#[test]
fn dead_source_sends_admitted_unfinished_successors_into_the_cascade() {
    // `join` is already `Ready` (admitted, start operation pending) when its
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
    assert_eq!(plan.operations[0].id.task.node.as_str(), "join");
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
    // The plan only schedules the skip; the operation owns the state change
    // against the authoritative state.
    assert!(plan.state_changes.is_empty());
    assert_eq!(plan.operations.len(), 1);
    assert_eq!(plan.operations[0].kind, OperationKind::SkipTask);
    assert_eq!(plan.operations[0].id.task.node.as_str(), "b");
    assert_eq!(
        plan.operations[0].id.sequence, 0,
        "no prior operations on b"
    );
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

#[test]
fn readiness_of_a_task_without_predecessors_is_ready() {
    let graph = graph();
    let snapshot = snapshot(TaskState::Completed, TaskState::Completed);
    // `root` has children but no predecessors.
    assert_eq!(
        readiness(&graph, &snapshot, &id("root"), JoinPolicy::AllSucceeded).unwrap(),
        JoinReadiness::Ready
    );
    // A predecessor missing from the aggregate is corrupt, not ready.
    let mut corrupt = snapshot.clone();
    corrupt.tasks.remove(&id("a"));
    assert!(matches!(
        readiness(&graph, &corrupt, &id("join"), JoinPolicy::AllSucceeded),
        Err(PropagationError::TaskNotFound(_))
    ));
}

#[test]
fn a_non_terminal_source_is_refused() {
    let mut snapshot = snapshot(TaskState::Running, TaskState::Running);
    snapshot.tasks.get_mut(&id("a")).unwrap().state = TaskState::Running;
    assert!(matches!(
        Propagate::plan(
            (&graph(), &snapshot),
            &command("a", JoinPolicy::AllSucceeded)
        ),
        Err(PropagationError::SourceNotTerminal { .. })
    ));
}

#[test]
fn a_flow_naming_an_uninstantiated_node_is_refused() {
    let mut command = command("a", JoinPolicy::AllSucceeded);
    command.flow = Some(FlowTarget {
        node: "ghost".into(),
        reopen: false,
        auto: true,
        vars: Vec::new(),
    });
    assert!(matches!(
        Propagate::plan(
            (
                &graph(),
                &snapshot(TaskState::Completed, TaskState::Completed)
            ),
            &command
        ),
        Err(PropagationError::FlowTargetNotInstantiated(_))
    ));
}

/// A reopen whose inherited predecessor is still in flight creates the new
/// generation `Created` (its predecessor's own propagation admits it later)
/// and carries the declared vars onto its scope.
#[test]
fn a_reopen_with_a_live_predecessor_waits_as_created() {
    let snapshot = ProcessSnapshot {
        process_id: "p1".into(),
        version: ProcessVersion {
            graph: 4,
            process: 4,
            operations: 2,
        },
        tasks: HashMap::from([
            (
                id("src"),
                TaskSnapshot {
                    model_node: NodeId::parse("src").unwrap(),
                    parent: None,
                    predecessors: vec![],
                    state: TaskState::Completed,
                    revision: 0,
                },
            ),
            (
                id("target"),
                TaskSnapshot {
                    model_node: NodeId::parse("target").unwrap(),
                    parent: None,
                    predecessors: vec![id("other")],
                    state: TaskState::Completed,
                    revision: 2,
                },
            ),
            (
                id("other"),
                TaskSnapshot {
                    model_node: NodeId::parse("other").unwrap(),
                    parent: None,
                    predecessors: vec![],
                    state: TaskState::Running,
                    revision: 0,
                },
            ),
        ]),
        variables: BTreeMap::new(),
        operations: HashMap::new(),
        outbox: HashMap::new(),
    };
    let graph = {
        let mut b = TaskGraphBuilder::new("p1", 4).unwrap();
        for name in ["src", "target", "other"] {
            b.add_node(TaskNode {
                id: id(name),
                model_node: NodeId::parse(name).unwrap(),
                state: TaskState::Created,
                revision: 0,
            })
            .unwrap();
        }
        b.set_root(id("src")).unwrap();
        b.add_child(id("src"), id("target")).unwrap();
        b.add_child(id("src"), id("other")).unwrap();
        b.add_successor(id("src"), id("target")).unwrap();
        b.build().unwrap()
    };
    let mut command = command("src", JoinPolicy::AllSucceeded);
    command.flow = Some(FlowTarget {
        node: "target".into(),
        reopen: true,
        auto: false,
        vars: vec![("declared".into(), "value".into())],
    });
    let plan = Propagate::plan((&graph, &snapshot), &command).unwrap();
    let create = plan
        .task_creates
        .iter()
        .find(|create| create.task.node.as_str() == "target")
        .expect("the flow re-creates the node");
    assert_eq!(create.task, TaskId::new("target", 1).unwrap());
    assert_eq!(
        create.initial_state,
        TaskState::Created,
        "a live predecessor admits the re-entry later"
    );
    assert!(
        plan.operations.iter().all(|op| op.id.task != create.task),
        "no start operation before its predecessor settles"
    );
    assert!(
        plan.variable_writes.iter().any(|write| {
            write.scope == create.task && write.name == "auto" && write.value == "0"
        })
    );
    assert!(plan.variable_writes.iter().any(|write| {
        write.scope == create.task && write.name == "declared" && write.value == "value"
    }));
}

#[test]
fn an_aborted_source_aborts_a_running_successor() {
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
    let mut snapshot = snapshot(TaskState::Aborted, TaskState::Running);
    snapshot.tasks.get_mut(&id("b")).unwrap().state = TaskState::Running;
    let plan =
        Propagate::plan((&graph, &snapshot), &command("a", JoinPolicy::AllSucceeded)).unwrap();
    assert_eq!(plan.operations.len(), 1);
    assert_eq!(plan.operations[0].kind, OperationKind::AbortTask);
}

#[test]
fn the_propagation_envelope_must_match_the_snapshot() {
    let mut cmd = command("a", JoinPolicy::AllSucceeded);
    cmd.process_id = "other".into();
    assert_eq!(
        Propagate::plan(
            (
                &graph(),
                &snapshot(TaskState::Completed, TaskState::Completed)
            ),
            &cmd
        ),
        Err(PropagationError::WrongProcess)
    );

    let mut cmd = command("a", JoinPolicy::AllSucceeded);
    cmd.process_id = "p1".into();
    let mut stale = snapshot(TaskState::Completed, TaskState::Completed);
    stale.version.graph = 5;
    assert_eq!(
        Propagate::plan((&graph(), &stale), &cmd),
        Err(PropagationError::GraphVersionConflict {
            graph: 4,
            snapshot: 5
        })
    );

    let mut cmd = command("a", JoinPolicy::AllSucceeded);
    cmd.source_operation.id.task = id("b");
    assert_eq!(
        Propagate::plan(
            (
                &graph(),
                &snapshot(TaskState::Completed, TaskState::Completed)
            ),
            &cmd
        ),
        Err(PropagationError::SourceOperationTargetsDifferentTask)
    );

    let mut cmd = command("a", JoinPolicy::AllSucceeded);
    cmd.task = id("elsewhere");
    cmd.source_operation.id.task = id("elsewhere");
    assert!(matches!(
        Propagate::plan(
            (
                &graph(),
                &snapshot(TaskState::Completed, TaskState::Completed)
            ),
            &cmd
        ),
        Err(PropagationError::TaskNotInGraph(_))
    ));
}

#[test]
fn propagation_errors_render() {
    for error in [
        PropagationError::WrongProcess,
        PropagationError::GraphVersionConflict {
            graph: 1,
            snapshot: 2,
        },
        PropagationError::SourceOperationTargetsDifferentTask,
        PropagationError::TaskNotInGraph(id("a")),
        PropagationError::TaskNotFound(id("a")),
        PropagationError::SourceNotTerminal {
            task: id("a"),
            state: TaskState::Running,
        },
        PropagationError::FlowTargetNotInstantiated("x".into()),
        PropagationError::EmptyFlowTarget,
        PropagationError::GenerationOverflow,
        PropagationError::InvalidPlan("bad".into()),
    ] {
        assert!(error.to_string().starts_with("propagation error:"));
    }
}

/// A reopen whose inherited predecessors have all settled is admitted
/// immediately (`Ready` + its start operation), and its scope carries the
/// source's user variables under the inherited parent.
#[test]
fn a_reopen_with_settled_predecessors_is_admitted_and_carries_parent_data() {
    let snapshot = ProcessSnapshot {
        process_id: "p1".into(),
        version: ProcessVersion {
            graph: 4,
            process: 4,
            operations: 2,
        },
        tasks: HashMap::from([
            (
                id("src"),
                TaskSnapshot {
                    model_node: NodeId::parse("src").unwrap(),
                    parent: None,
                    predecessors: vec![],
                    state: TaskState::Completed,
                    revision: 0,
                },
            ),
            (
                id("target"),
                TaskSnapshot {
                    model_node: NodeId::parse("target").unwrap(),
                    parent: Some(id("parent")),
                    predecessors: vec![id("done")],
                    state: TaskState::Completed,
                    revision: 2,
                },
            ),
            (
                id("done"),
                TaskSnapshot {
                    model_node: NodeId::parse("done").unwrap(),
                    parent: None,
                    predecessors: vec![],
                    state: TaskState::Completed,
                    revision: 0,
                },
            ),
            (
                id("parent"),
                TaskSnapshot {
                    model_node: NodeId::parse("parent").unwrap(),
                    parent: None,
                    predecessors: vec![],
                    state: TaskState::Waiting,
                    revision: 0,
                },
            ),
        ]),
        variables: BTreeMap::from([
            ((id("src"), "answer".to_string()), "\"42\"".to_string()),
            ((id("src"), "auto".to_string()), "1".to_string()),
            ((id("src"), "error.code".to_string()), "\"E\"".to_string()),
            ((id("src"), "__private".to_string()), "true".to_string()),
        ]),
        operations: HashMap::new(),
        outbox: HashMap::new(),
    };
    let graph = {
        let mut b = TaskGraphBuilder::new("p1", 4).unwrap();
        for name in ["src", "target", "done", "parent"] {
            b.add_node(TaskNode {
                id: id(name),
                model_node: NodeId::parse(name).unwrap(),
                state: TaskState::Created,
                revision: 0,
            })
            .unwrap();
        }
        b.set_root(id("src")).unwrap();
        b.add_child(id("src"), id("target")).unwrap();
        b.add_child(id("src"), id("done")).unwrap();
        b.add_child(id("src"), id("parent")).unwrap();
        b.add_successor(id("src"), id("target")).unwrap();
        b.build().unwrap()
    };
    let mut command = command("src", JoinPolicy::AllSucceeded);
    command.flow = Some(FlowTarget {
        node: "target".into(),
        reopen: true,
        auto: true,
        vars: vec![("declared".into(), "value".into())],
    });
    let plan = Propagate::plan((&graph, &snapshot), &command).unwrap();
    let create = plan
        .task_creates
        .iter()
        .find(|create| create.task.node.as_str() == "target")
        .expect("the flow re-creates the node");
    assert_eq!(create.initial_state, TaskState::Ready);
    assert_eq!(create.parent, Some(id("parent")));
    assert_eq!(create.predecessors, vec![id("done")]);
    assert!(
        plan.operations
            .iter()
            .any(|op| { op.id.task == create.task && op.kind == OperationKind::ExecuteTask }),
        "the settled re-entry is admitted with its start operation"
    );
    assert!(
        plan.variable_writes
            .iter()
            .any(|write| write.scope == id("parent") && write.name == "answer"),
        "the source's user data travels to the inherited parent scope"
    );
    assert!(
        plan.variable_writes
            .iter()
            .all(|write| write.name != "auto" || write.scope != id("parent")),
        "the auto decision is not copied as user data"
    );
    assert!(
        plan.variable_writes
            .iter()
            .all(|write| write.name != "error.code" && !write.name.starts_with("__")),
        "private and failure facts are carried by their own walks"
    );
}

#[test]
fn the_envelope_checks_each_process_identity_independently() {
    // The graph agrees, the snapshot does not.
    let mut foreign = snapshot(TaskState::Completed, TaskState::Completed);
    foreign.process_id = "other".into();
    assert_eq!(
        Propagate::plan(
            (&graph(), &foreign),
            &command("a", JoinPolicy::AllSucceeded)
        ),
        Err(PropagationError::WrongProcess)
    );

    // The source operation names a foreign process but the right task.
    let mut command = command("a", JoinPolicy::AllSucceeded);
    command.source_operation.id.process_id = "other".into();
    assert_eq!(
        Propagate::plan(
            (
                &graph(),
                &snapshot(TaskState::Completed, TaskState::Completed)
            ),
            &command
        ),
        Err(PropagationError::SourceOperationTargetsDifferentTask)
    );
}

#[test]
fn an_aborted_source_skips_an_admitted_non_running_successor() {
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
    let mut snapshot = snapshot(TaskState::Aborted, TaskState::Ready);
    snapshot.tasks.get_mut(&id("b")).unwrap().state = TaskState::Ready;
    let plan =
        Propagate::plan((&graph, &snapshot), &command("a", JoinPolicy::AllSucceeded)).unwrap();
    assert_eq!(plan.operations.len(), 1);
    assert_eq!(plan.operations[0].kind, OperationKind::SkipTask);
}
