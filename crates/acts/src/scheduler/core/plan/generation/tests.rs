use super::*;

use crate::scheduler::core::commit::aggregate::TaskSnapshot;
use crate::scheduler::core::model::model_graph::NodeId;
use crate::scheduler::core::model::operation::ClaimToken;
use crate::scheduler::core::model::transition_plan::ProcessVersion;
use crate::scheduler::core::plan::ActPlan;
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
        cancels_forward: false,
        vars: HashMap::new(),
        source_operation: OperationCommit {
            id: OperationId {
                process_id: "p1".into(),
                task: id("source", 1),
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
    let plan = Reenter::plan((&graph(), &snapshot()), &command()).unwrap();
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
    assert_eq!(plan.operations[0].id.task.node.as_str(), "source");
    assert_eq!(plan.operations[0].id.task.generation, 2);
    let target = plan
        .task_creates
        .iter()
        .find(|create| create.task == id("target", 2))
        .unwrap();
    assert_eq!(target.predecessors, vec![id("source", 2)]);
}

#[test]
fn reentry_seeds_the_declared_model_vars_on_every_recreated_generation() {
    let mut command = command();
    command.vars.insert(
        "source".into(),
        vec![
            ("flag".into(), "true".into()),
            // A declared var the engine also owns must not duplicate the
            // auto write: the plan validator refuses duplicate writes.
            ("auto".into(), "0".into()),
        ],
    );
    command
        .vars
        .insert("target".into(), vec![("amount".into(), "42".into())]);
    let plan = Reenter::plan((&graph(), &snapshot()), &command).unwrap();
    let write = |scope: TaskId, name: &str| {
        plan.variable_writes
            .iter()
            .find(|write| write.scope == scope && write.name == name)
            .map(|write| write.value.as_str())
    };
    assert_eq!(write(id("source", 2), "flag"), Some("true"));
    assert_eq!(
        write(id("source", 2), "auto"),
        Some("1"),
        "the auto decision keeps its own write"
    );
    assert_eq!(write(id("target", 2), "amount"), Some("42"));
}

#[test]
fn non_reenterable_source_cannot_reenter() {
    let mut snapshot = snapshot();
    snapshot.tasks.get_mut(&id("source", 1)).unwrap().state = TaskState::Waiting;
    assert!(matches!(
        Reenter::plan((&graph(), &snapshot), &command()),
        Err(GenerationError::SourceNotReenterable { .. })
    ));
}

#[test]
fn cancelled_source_reenters_and_recreates_the_forward_closure() {
    let mut snapshot = snapshot();
    // The cancel origin keeps the finished state it already held.
    snapshot.tasks.get_mut(&id("source", 1)).unwrap().state = TaskState::Completed;
    // The old target generation was skipped as the work behind the
    // cancelled front.
    snapshot.tasks.get_mut(&id("target", 1)).unwrap().state = TaskState::Skipped;
    let mut command = command();
    command.cancels_forward = true;
    let plan = Reenter::plan((&graph(), &snapshot), &command).unwrap();
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
    assert_eq!(plan.operations[0].id.task.node.as_str(), "source");
    assert_eq!(plan.operations[0].id.task.generation, 2);
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
    let plan = Reenter::plan((&graph, &snapshot), &command).unwrap();
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
    assert_eq!(plan.operations[0].id.task.node.as_str(), "target");
    assert_eq!(plan.operations[0].id.task.generation, 2);
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
        cancels_forward: false,
        vars: HashMap::new(),
        source_operation: OperationCommit {
            id: OperationId {
                process_id: "p1".into(),
                task: id("stray", 0),
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
        Reenter::plan((&graph, &snapshot), &command),
        Err(GenerationError::SourceOutsideRedoClosure { .. })
    ));
}

#[test]
fn the_command_envelope_must_match_the_snapshot() {
    let mut wrong_process = command();
    wrong_process.process_id = "other".into();
    assert_eq!(
        Reenter::plan((&graph(), &snapshot()), &wrong_process),
        Err(GenerationError::WrongProcess)
    );

    let mut wrong_target = command();
    wrong_target.source_operation.id.task = id("target", 1);
    assert_eq!(
        Reenter::plan((&graph(), &snapshot()), &wrong_target),
        Err(GenerationError::SourceOperationTargetsDifferentTask)
    );

    let mut empty_target = command();
    empty_target.target_task_id = "  ".into();
    assert_eq!(
        Reenter::plan((&graph(), &snapshot()), &empty_target),
        Err(GenerationError::EmptyTargetTaskId)
    );

    let mut never = command();
    never.target_task_id = "ghost".into();
    assert!(matches!(
        Reenter::plan((&graph(), &snapshot()), &never),
        Err(GenerationError::TargetNeverInstantiated(_))
    ));
}

/// A diamond closure reaches one recreation from two paths: the breadth-first
/// walk must visit it once, not enqueue it twice.
#[test]
fn a_diamond_closure_visits_each_execution_once() {
    use crate::scheduler::core::model::task_graph::TaskGraphBuilder;
    use crate::scheduler::core::model::task_graph::TaskNode;
    let mut snapshot = ProcessSnapshot {
        process_id: "p1".into(),
        version: ProcessVersion {
            graph: 3,
            process: 8,
            operations: 6,
        },
        tasks: HashMap::new(),
        variables: BTreeMap::new(),
        operations: HashMap::new(),
        outbox: HashMap::new(),
    };
    for name in ["root", "a", "b", "join"] {
        snapshot.tasks.insert(
            id(name, 0),
            TaskSnapshot {
                model_node: NodeId::parse(name).unwrap(),
                parent: None,
                predecessors: vec![],
                state: TaskState::Completed,
                revision: 0,
            },
        );
    }
    snapshot.tasks.get_mut(&id("root", 0)).unwrap().state = TaskState::Backed;
    let mut builder = TaskGraphBuilder::new("p1", 3).unwrap();
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
    builder.set_root(id("root", 0)).unwrap();
    builder.add_child(id("root", 0), id("a", 0)).unwrap();
    builder.add_child(id("root", 0), id("b", 0)).unwrap();
    builder.add_child(id("a", 0), id("join", 0)).unwrap();
    builder.add_successor(id("root", 0), id("a", 0)).unwrap();
    builder.add_successor(id("root", 0), id("b", 0)).unwrap();
    builder.add_successor(id("a", 0), id("join", 0)).unwrap();
    builder.add_successor(id("b", 0), id("join", 0)).unwrap();
    let graph = builder.build().unwrap();
    let mut command = command();
    command.source = id("root", 0);
    command.target_task_id = "root".into();
    command.source_operation.id.task = id("root", 0);
    let plan = Reenter::plan((&graph, &snapshot), &command).unwrap();
    let created: Vec<TaskId> = plan
        .task_creates
        .iter()
        .map(|create| create.task.clone())
        .collect();
    assert_eq!(
        created,
        vec![id("root", 1), id("a", 1), id("b", 1), id("join", 1)]
    );
}

#[test]
fn generation_errors_render() {
    for error in [
        GenerationError::WrongProcess,
        GenerationError::TaskNotFound(id("a", 0)),
        GenerationError::SourceNotReenterable {
            source: id("a", 0),
            state: TaskState::Running,
        },
        GenerationError::SourceOperationTargetsDifferentTask,
        GenerationError::EmptyTargetTaskId,
        GenerationError::TargetNeverInstantiated("ghost".into()),
        GenerationError::SourceOutsideRedoClosure {
            source: id("a", 0),
            target: "b".into(),
        },
        GenerationError::GenerationOverflow,
        GenerationError::InvalidPlan("bad".into()),
    ] {
        assert!(error.to_string().starts_with("generation error:"));
    }
}

#[test]
fn the_source_operation_must_belong_to_the_command_process() {
    let mut command = command();
    command.source_operation.id.process_id = "other".into();
    assert_eq!(
        Reenter::plan((&graph(), &snapshot()), &command),
        Err(GenerationError::SourceOperationTargetsDifferentTask)
    );
}

/// A node with several live instances (a fan-out's parallel items share one
/// node id) maps every closed-over instance onto its own fresh generation:
/// mapping both onto one generation collides in the plan and refuses the
/// whole rewind.
#[test]
fn multi_instance_nodes_recreate_one_generation_each() {
    use crate::scheduler::core::model::task_graph::{TaskGraphBuilder, TaskNode};
    let mut snapshot = ProcessSnapshot {
        process_id: "p1".into(),
        version: ProcessVersion {
            graph: 3,
            process: 8,
            operations: 6,
        },
        tasks: HashMap::new(),
        variables: BTreeMap::new(),
        operations: HashMap::new(),
        outbox: HashMap::new(),
    };
    // The rewind source, and one forward node with two live instances — the
    // shape a candidates fan-out leaves behind.
    snapshot.tasks.insert(
        id("root", 0),
        TaskSnapshot {
            model_node: NodeId::parse("root").unwrap(),
            parent: None,
            predecessors: vec![],
            state: TaskState::Backed,
            revision: 0,
        },
    );
    for generation in [0, 1] {
        snapshot.tasks.insert(
            id("branch", generation),
            TaskSnapshot {
                model_node: NodeId::parse("branch").unwrap(),
                parent: None,
                predecessors: vec![id("root", 0)],
                state: TaskState::Waiting,
                revision: 0,
            },
        );
    }
    let mut builder = TaskGraphBuilder::new("p1", 3).unwrap();
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
    builder.set_root(id("root", 0)).unwrap();
    // The instances hang off the root as its children and flow forward as
    // successors — the shape a fan-out leaves in the execution graph. The
    // reachability walk follows child edges; the closure follows successors.
    for generation in [0, 1] {
        builder
            .add_child(id("root", 0), id("branch", generation))
            .unwrap();
        builder
            .add_successor(id("root", 0), id("branch", generation))
            .unwrap();
    }
    let graph = builder.build().unwrap();
    let mut command = command();
    command.source = id("root", 0);
    command.target_task_id = "root".into();
    command.source_operation.id.task = id("root", 0);
    // The plan commits at all: the two instances no longer collide on one
    // generation.
    let plan = Reenter::plan((&graph, &snapshot), &command).unwrap();
    let created: Vec<TaskId> = plan
        .task_creates
        .iter()
        .map(|create| create.task.clone())
        .collect();
    assert_eq!(
        created,
        vec![id("root", 1), id("branch", 2), id("branch", 3)],
        "each closed-over instance takes the generation after the last: {created:?}"
    );
    for generation in [2, 3] {
        let branch = plan
            .task_creates
            .iter()
            .find(|create| create.task == id("branch", generation))
            .unwrap();
        assert_eq!(
            branch.predecessors,
            vec![id("root", 1)],
            "the instance's edge remaps onto the re-entered root"
        );
    }
}

/// Back walks from the source up to the named step and closes what it
/// passes: the running containers on the path close `Backed`, the sibling
/// branches beside the path close `Skipped` (their subtrees included); the
/// source generation is already `Backed` by the decision, and the workflow
/// root is never walked.
#[test]
fn back_closes_the_walked_path_and_skips_sibling_branches() {
    use crate::scheduler::core::model::task_graph::{TaskGraphBuilder, TaskNode};
    let mut snapshot = ProcessSnapshot {
        process_id: "p1".into(),
        version: ProcessVersion {
            graph: 3,
            process: 8,
            operations: 6,
        },
        tasks: HashMap::new(),
        variables: BTreeMap::new(),
        operations: HashMap::new(),
        outbox: HashMap::new(),
    };
    let task = |snapshot: &mut ProcessSnapshot, id: TaskId, parent: Option<TaskId>, state| {
        let model_node = NodeId::parse(id.node.as_str()).unwrap();
        snapshot.tasks.insert(
            id,
            TaskSnapshot {
                model_node,
                parent,
                predecessors: vec![],
                state,
                revision: 0,
            },
        );
    };
    // root -> prepare (the redo target) and root -> approve (running);
    // approve fans out into two items, one per vote.
    task(&mut snapshot, id("root", 0), None, TaskState::Completed);
    task(
        &mut snapshot,
        id("prepare", 0),
        Some(id("root", 0)),
        TaskState::Completed,
    );
    task(
        &mut snapshot,
        id("approve", 0),
        Some(id("root", 0)),
        TaskState::Running,
    );
    task(
        &mut snapshot,
        id("item", 0),
        Some(id("approve", 0)),
        TaskState::Running,
    );
    task(
        &mut snapshot,
        id("item", 1),
        Some(id("approve", 0)),
        TaskState::Running,
    );
    // The source: the first vote, already closed by the decision.
    task(
        &mut snapshot,
        id("vote", 0),
        Some(id("item", 0)),
        TaskState::Backed,
    );
    // Its sibling branch: the second vote, still waiting.
    task(
        &mut snapshot,
        id("vote", 1),
        Some(id("item", 1)),
        TaskState::Waiting,
    );
    let mut builder = TaskGraphBuilder::new("p1", 3).unwrap();
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
    builder.set_root(id("root", 0)).unwrap();
    for (parent, child) in [
        (id("root", 0), id("prepare", 0)),
        (id("root", 0), id("approve", 0)),
        (id("approve", 0), id("item", 0)),
        (id("approve", 0), id("item", 1)),
        (id("item", 0), id("vote", 0)),
        (id("item", 1), id("vote", 1)),
    ] {
        builder.add_child(parent, child).unwrap();
    }
    builder
        .add_successor(id("prepare", 0), id("approve", 0))
        .unwrap();
    builder
        .add_successor(id("approve", 0), id("item", 0))
        .unwrap();
    builder
        .add_successor(id("approve", 0), id("item", 1))
        .unwrap();
    builder.add_successor(id("item", 0), id("vote", 0)).unwrap();
    builder.add_successor(id("item", 1), id("vote", 1)).unwrap();
    let graph = builder.build().unwrap();
    let mut command = command();
    command.source = id("vote", 0);
    command.target_task_id = "prepare".into();
    command.source_operation.id.task = id("vote", 0);
    let plan = Reenter::plan((&graph, &snapshot), &command).unwrap();
    let changes: Vec<(TaskId, TaskState, TaskState)> = plan
        .state_changes
        .iter()
        .map(|change| (change.task.clone(), change.from, change.to))
        .collect();
    assert_eq!(
        changes,
        vec![
            // The walked path: the container above the source and the item
            // holding it (in the closure's breadth-first order).
            (id("approve", 0), TaskState::Running, TaskState::Backed),
            (id("item", 0), TaskState::Running, TaskState::Backed),
            // The sibling branch beside the path, its vote included.
            (id("item", 1), TaskState::Running, TaskState::Skipped),
            (id("vote", 1), TaskState::Waiting, TaskState::Skipped),
        ],
        "the rewind closes the path and the sibling branches: {changes:?}"
    );
    // The fresh generations: the target runs again and the whole forward
    // closure is recreated beside the closed one.
    let created: Vec<TaskId> = plan
        .task_creates
        .iter()
        .map(|create| create.task.clone())
        .collect();
    assert_eq!(
        created,
        vec![
            id("prepare", 1),
            id("approve", 1),
            id("item", 2),
            id("item", 3),
            id("vote", 2),
            id("vote", 3),
        ],
        "the recreated closure maps every closed-over instance: {created:?}"
    );
    assert_eq!(
        plan.task_creates[0].initial_state,
        TaskState::Ready,
        "the named redo step runs again immediately"
    );
}

/// Cancel marks the work ahead of the act it was issued from: the nearest
/// admitted forward task closes `Cancelled`, everything below it — its own
/// children included — closes `Skipped`, and the act itself keeps the state
/// it already held (it is the origin of the rewind, re-entered as a fresh
/// generation).
#[test]
fn cancel_closes_the_running_work_ahead() {
    use crate::scheduler::core::model::task_graph::{TaskGraphBuilder, TaskNode};
    let mut snapshot = ProcessSnapshot {
        process_id: "p1".into(),
        version: ProcessVersion {
            graph: 3,
            process: 8,
            operations: 6,
        },
        tasks: HashMap::new(),
        variables: BTreeMap::new(),
        operations: HashMap::new(),
        outbox: HashMap::new(),
    };
    let task = |snapshot: &mut ProcessSnapshot, id: TaskId, parent: Option<TaskId>, state| {
        let model_node = NodeId::parse(id.node.as_str()).unwrap();
        snapshot.tasks.insert(
            id,
            TaskSnapshot {
                model_node,
                parent,
                predecessors: vec![],
                state,
                revision: 0,
            },
        );
    };
    // root -> step (the cancel origin, finished) -> next (running)
    // -> leaf (waiting).
    task(&mut snapshot, id("root", 0), None, TaskState::Completed);
    task(
        &mut snapshot,
        id("step", 0),
        Some(id("root", 0)),
        TaskState::Completed,
    );
    task(
        &mut snapshot,
        id("next", 0),
        Some(id("step", 0)),
        TaskState::Running,
    );
    task(
        &mut snapshot,
        id("leaf", 0),
        Some(id("next", 0)),
        TaskState::Waiting,
    );
    let mut builder = TaskGraphBuilder::new("p1", 3).unwrap();
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
    builder.set_root(id("root", 0)).unwrap();
    for (parent, child) in [
        (id("root", 0), id("step", 0)),
        (id("step", 0), id("next", 0)),
        (id("next", 0), id("leaf", 0)),
    ] {
        builder.add_child(parent.clone(), child.clone()).unwrap();
        builder.add_successor(parent, child).unwrap();
    }
    let graph = builder.build().unwrap();
    let mut command = command();
    command.source = id("step", 0);
    command.target_task_id = "step".into();
    command.cancels_forward = true;
    command.source_operation.id.task = id("step", 0);
    let plan = Reenter::plan((&graph, &snapshot), &command).unwrap();
    let changes: Vec<(TaskId, TaskState, TaskState)> = plan
        .state_changes
        .iter()
        .map(|change| (change.task.clone(), change.from, change.to))
        .collect();
    assert_eq!(
        changes,
        vec![
            (id("next", 0), TaskState::Running, TaskState::Cancelled),
            (id("leaf", 0), TaskState::Waiting, TaskState::Skipped),
        ],
        "the admitted front closes cancelled, its subtree skipped: {changes:?}"
    );
    let created: Vec<TaskId> = plan
        .task_creates
        .iter()
        .map(|create| create.task.clone())
        .collect();
    assert_eq!(
        created,
        vec![id("step", 1), id("next", 1), id("leaf", 1)],
        "the cancelled step re-enters with its forward closure: {created:?}"
    );
}
