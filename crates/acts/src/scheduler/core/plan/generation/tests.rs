use super::*;
use crate::scheduler::core::commit::aggregate::TaskSnapshot;
use crate::scheduler::core::model::model_graph::NodeId;
use crate::scheduler::core::model::operation::ClaimToken;
use crate::scheduler::core::model::transition_plan::ProcessVersion;
use std::collections::{BTreeMap, HashMap};

fn id(task: &str, generation: u64) -> TaskId {
    TaskId::new(task, generation).unwrap()
}
fn snapshot() -> ProcessSnapshot {
    ProcessSnapshot {
        process_id: "p1".into(),
        version: ProcessVersion {
            graph: 3,
            process: 8,
            operations: 6,
        },
        tasks: HashMap::from([
            (
                id("source", 1),
                TaskSnapshot {
                    model_node: NodeId::parse("source").unwrap(),
                    parent: None,
                    predecessors: vec![],
                    state: TaskState::Backed,
                    revision: 4,
                },
            ),
            (
                id("target", 0),
                TaskSnapshot {
                    model_node: NodeId::parse("target-node").unwrap(),
                    parent: None,
                    predecessors: vec![],
                    state: TaskState::Completed,
                    revision: 3,
                },
            ),
            (
                id("target", 1),
                TaskSnapshot {
                    model_node: NodeId::parse("target-node").unwrap(),
                    parent: None,
                    predecessors: vec![id("source", 1)],
                    state: TaskState::Completed,
                    revision: 3,
                },
            ),
        ]),
        variables: BTreeMap::new(),
        operations: HashMap::new(),
        outbox: HashMap::new(),
    }
}
/// The execution graph of [`snapshot`]: source gen1 -> target gen1.
fn graph() -> TaskGraph {
    use crate::scheduler::core::model::task_graph::TaskGraphBuilder;
    use crate::scheduler::core::model::task_graph::TaskNode;
    let snapshot = snapshot();
    let mut builder = TaskGraphBuilder::new(&snapshot.process_id, snapshot.version.graph).unwrap();
    for (exec_id, task) in &snapshot.tasks {
        builder
            .add_node(TaskNode {
                id: exec_id.clone(),
                model_node: task.model_node.clone(),
                state: task.state,
                revision: task.revision,
            })
            .unwrap();
    }
    builder.set_root(id("source", 1)).unwrap();
    builder.add_child(id("source", 1), id("target", 0)).unwrap();
    builder.add_child(id("source", 1), id("target", 1)).unwrap();
    // The propagation edge mirrors the target's predecessor list: the
    // live generation hangs off the cancelled source generation.
    builder
        .add_successor(id("source", 1), id("target", 1))
        .unwrap();
    builder.build().unwrap()
}
fn command() -> ReenterCommand {
    ReenterCommand {
        process_id: "p1".into(),
        source: id("source", 1),
        // Cancel re-enters the source itself; the target names the
        // re-entered task.
        target_task_id: "source".into(),
        source_operation: OperationCommit {
            id: OperationId {
                process_id: "p1".into(),
                node: "source".into(),
                generation: 1,
                sequence: 8,
            },
            claim: ClaimToken {
                owner: "runtime-a".into(),
                fence: 7,
                lease_until_ms: i64::MAX,
                heartbeat_revision: 0,
            },
        },
    }
}

#[test]
fn reentry_creates_new_generation_and_never_mutates_history() {
    let plan = GenerationPlanner::plan_reentry(&graph(), &snapshot(), &command()).unwrap();
    assert!(plan.state_changes.is_empty());
    // The re-entered source generation runs again; the downstream target
    // is recreated one generation up, waiting for the source's
    // propagation.
    let created: Vec<TaskId> = plan
        .task_creates
        .iter()
        .map(|create| create.task.clone())
        .collect();
    assert_eq!(created, vec![id("source", 2), id("target", 2)]);
    assert_eq!(plan.task_creates[0].initial_state, TaskState::Ready);
    assert_eq!(plan.operations[0].id.node, "source");
    assert_eq!(plan.operations[0].id.generation, 2);
    let target = plan
        .task_creates
        .iter()
        .find(|create| create.task == id("target", 2))
        .unwrap();
    assert_eq!(target.predecessors, vec![id("source", 2)]);
}

#[test]
fn non_reenterable_source_cannot_reenter() {
    let mut snapshot = snapshot();
    snapshot.tasks.get_mut(&id("source", 1)).unwrap().state = TaskState::Waiting;
    assert!(matches!(
        GenerationPlanner::plan_reentry(&graph(), &snapshot, &command()),
        Err(GenerationError::SourceNotReenterable { .. })
    ));
}

#[test]
fn cancelled_source_reenters_and_recreates_the_forward_closure() {
    let mut snapshot = snapshot();
    snapshot.tasks.get_mut(&id("source", 1)).unwrap().state = TaskState::Cancelled;
    // The old target generation was skipped by the cascade that followed
    // the cancellation.
    snapshot.tasks.get_mut(&id("target", 1)).unwrap().state = TaskState::Skipped;
    let plan = GenerationPlanner::plan_reentry(&graph(), &snapshot, &command()).unwrap();
    // Both the target and the forward source generation are recreated.
    let created: Vec<TaskId> = plan
        .task_creates
        .iter()
        .map(|create| create.task.clone())
        .collect();
    assert_eq!(created, vec![id("source", 2), id("target", 2)]);
    // The recreated source carries an ExecuteTask; the recreated target
    // waits for the source's propagation.
    assert_eq!(plan.operations.len(), 1);
    assert_eq!(plan.operations[0].id.node, "source");
    assert_eq!(plan.operations[0].id.generation, 2);
    // The recreated target's predecessor edge points at the recreated
    // source, not at the dead generation.
    let target = plan
        .task_creates
        .iter()
        .find(|create| create.task == id("target", 2))
        .unwrap();
    assert_eq!(target.predecessors, vec![id("source", 2)]);
}

#[test]
fn back_reenters_the_named_step_and_carries_the_backed_source() {
    // The source is the Backed downstream execution; the client named
    // `target` as the redo point. The closure roots at the target's
    // latest generation and must carry the source with it.
    let snapshot = ProcessSnapshot {
        process_id: "p1".into(),
        version: ProcessVersion {
            graph: 3,
            process: 8,
            operations: 6,
        },
        tasks: HashMap::from([
            (
                id("target", 1),
                TaskSnapshot {
                    model_node: NodeId::parse("target-node").unwrap(),
                    parent: None,
                    predecessors: vec![],
                    state: TaskState::Completed,
                    revision: 3,
                },
            ),
            (
                id("source", 1),
                TaskSnapshot {
                    model_node: NodeId::parse("source").unwrap(),
                    parent: None,
                    predecessors: vec![id("target", 1)],
                    state: TaskState::Backed,
                    revision: 4,
                },
            ),
        ]),
        variables: BTreeMap::new(),
        operations: HashMap::new(),
        outbox: HashMap::new(),
    };
    let graph = {
        let mut builder =
            crate::scheduler::core::model::task_graph::TaskGraphBuilder::new("p1", 3).unwrap();
        for (exec_id, task) in &snapshot.tasks {
            builder
                .add_node(crate::scheduler::core::model::task_graph::TaskNode {
                    id: exec_id.clone(),
                    model_node: task.model_node.clone(),
                    state: task.state,
                    revision: task.revision,
                })
                .unwrap();
        }
        builder.set_root(id("target", 1)).unwrap();
        builder.add_child(id("target", 1), id("source", 1)).unwrap();
        builder
            .add_successor(id("target", 1), id("source", 1))
            .unwrap();
        builder.build().unwrap()
    };
    let mut command = command();
    command.target_task_id = "target".into();
    let plan = GenerationPlanner::plan_reentry(&graph, &snapshot, &command).unwrap();
    let created: Vec<TaskId> = plan
        .task_creates
        .iter()
        .map(|create| create.task.clone())
        .collect();
    // The redo starts at the target's next generation; the backed source
    // is recreated behind it (the redo re-runs everything downstream).
    assert_eq!(created, vec![id("target", 2), id("source", 2)]);
    assert_eq!(
        plan.task_creates[0].initial_state,
        TaskState::Ready,
        "the named redo step runs again immediately"
    );
    assert_eq!(plan.operations[0].id.node, "target");
    assert_eq!(plan.operations[0].id.generation, 2);
    let source = plan
        .task_creates
        .iter()
        .find(|create| create.task == id("source", 2))
        .unwrap();
    assert_eq!(source.initial_state, TaskState::Created);
    assert_eq!(source.predecessors, vec![id("target", 2)]);
}

#[test]
fn back_to_a_step_outside_the_redo_closure_is_refused() {
    // A source with no path from the named step cannot be a redo. The
    // graph stays connected: one root with two children, and the named
    // `target` spawns a closure that misses the backed `stray`.
    let snapshot = ProcessSnapshot {
        process_id: "p1".into(),
        version: ProcessVersion {
            graph: 4,
            process: 9,
            operations: 7,
        },
        tasks: HashMap::from([
            (
                id("root", 0),
                TaskSnapshot {
                    model_node: NodeId::parse("root").unwrap(),
                    parent: None,
                    predecessors: vec![],
                    state: TaskState::Completed,
                    revision: 1,
                },
            ),
            (
                id("target", 1),
                TaskSnapshot {
                    model_node: NodeId::parse("target-node").unwrap(),
                    parent: Some(id("root", 0)),
                    predecessors: vec![],
                    state: TaskState::Completed,
                    revision: 3,
                },
            ),
            (
                id("stray", 0),
                TaskSnapshot {
                    model_node: NodeId::parse("stray").unwrap(),
                    parent: Some(id("root", 0)),
                    predecessors: vec![],
                    state: TaskState::Backed,
                    revision: 1,
                },
            ),
        ]),
        variables: BTreeMap::new(),
        operations: HashMap::new(),
        outbox: HashMap::new(),
    };
    let graph = {
        let mut builder =
            crate::scheduler::core::model::task_graph::TaskGraphBuilder::new("p1", 4).unwrap();
        for (exec_id, task) in &snapshot.tasks {
            builder
                .add_node(crate::scheduler::core::model::task_graph::TaskNode {
                    id: exec_id.clone(),
                    model_node: task.model_node.clone(),
                    state: task.state,
                    revision: task.revision,
                })
                .unwrap();
        }
        builder.set_root(id("root", 0)).unwrap();
        builder.add_child(id("root", 0), id("target", 1)).unwrap();
        builder.add_child(id("root", 0), id("stray", 0)).unwrap();
        builder.build().unwrap()
    };
    let command = ReenterCommand {
        process_id: "p1".into(),
        source: id("stray", 0),
        target_task_id: "target".into(),
        source_operation: OperationCommit {
            id: OperationId {
                process_id: "p1".into(),
                node: "stray".into(),
                generation: 0,
                sequence: 1,
            },
            claim: ClaimToken {
                owner: "runtime-a".into(),
                fence: 7,
                lease_until_ms: i64::MAX,
                heartbeat_revision: 0,
            },
        },
    };
    assert!(matches!(
        GenerationPlanner::plan_reentry(&graph, &snapshot, &command),
        Err(GenerationError::SourceOutsideRedoClosure { .. })
    ));
}
