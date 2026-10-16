use super::*;

#[test]
fn fanout_keeps_the_source_running_and_admits_entry_and_skip_children() {
    let (snapshot, command) = fanout_command(
        "step1",
        TaskState::Running,
        vec![
            child(
                "b1",
                ContainerChildPlan::Entry,
                vec![ContainerPredecessor::Source],
            ),
            child(
                "b2",
                ContainerChildPlan::Skip,
                vec![ContainerPredecessor::Source],
            ),
        ],
    );
    let plan = Fanout::plan(&snapshot, &command).unwrap();
    assert_eq!(plan.task_creates.len(), 2);
    assert!(
        plan.task_creates
            .iter()
            .all(|create| create.parent == Some(id("step1", 0)))
    );
    let entry = plan
        .task_creates
        .iter()
        .find(|create| create.task.node.as_str() == "b1")
        .unwrap();
    assert_eq!(entry.initial_state, TaskState::Ready);
    let skip = plan
        .task_creates
        .iter()
        .find(|create| create.task.node.as_str() == "b2")
        .unwrap();
    assert_eq!(skip.initial_state, TaskState::Ready);
    assert_eq!(plan.operations.len(), 2);
    assert!(
        plan.operations
            .iter()
            .any(|op| op.id.task.node.as_str() == "b1" && op.kind == OperationKind::ExecuteTask)
    );
    assert!(
        plan.operations
            .iter()
            .any(|op| op.id.task.node.as_str() == "b2" && op.kind == OperationKind::SkipTask)
    );
    assert_eq!(
        plan.state_changes.len(),
        0,
        "the running source stays running"
    );
    // The auto decision is durable state for the created children.
    assert_eq!(plan.variable_writes.len(), 2);
}

/// A container fanning out from `Waiting` — a step whose act the client
/// just answered — resumes `Running`, exactly like the legacy step whose
/// `next` runs after its act completed.
#[test]
fn fanout_resumes_a_waiting_source_into_running() {
    let (snapshot, command) = fanout_command(
        "step1",
        TaskState::Waiting,
        vec![child(
            "b1",
            ContainerChildPlan::Entry,
            vec![ContainerPredecessor::Source],
        )],
    );
    let plan = Fanout::plan(&snapshot, &command).unwrap();
    assert_eq!(plan.state_changes.len(), 1);
    let change = &plan.state_changes[0];
    assert_eq!(change.task, id("step1", 0));
    assert_eq!(change.from, TaskState::Waiting);
    assert_eq!(change.to, TaskState::Running);
}

#[test]
fn fanout_queues_chain_successors_behind_the_entry() {
    let (snapshot, command) = fanout_command(
        "b1",
        TaskState::Running,
        vec![
            child(
                "s1",
                ContainerChildPlan::Entry,
                vec![ContainerPredecessor::Source],
            ),
            child(
                "s2",
                ContainerChildPlan::Queued,
                vec![ContainerPredecessor::Sibling(0)],
            ),
        ],
    );
    let plan = Fanout::plan(&snapshot, &command).unwrap();
    assert_eq!(plan.task_creates.len(), 2);
    let queued = plan
        .task_creates
        .iter()
        .find(|create| create.task.node.as_str() == "s2")
        .unwrap();
    assert_eq!(queued.initial_state, TaskState::Created);
    assert_eq!(queued.predecessors, vec![id("s1", 0)]);
    // Only the entry receives an operation; propagation admits the rest.
    assert_eq!(plan.operations.len(), 1);
    assert_eq!(plan.operations[0].kind, OperationKind::ExecuteTask);
}

#[test]
fn empty_container_completes_and_propagates_in_the_same_operation() {
    let (snapshot, command) = fanout_command("b1", TaskState::Running, vec![]);
    let plan = Fanout::plan(&snapshot, &command).unwrap();
    assert!(plan.task_creates.is_empty());
    assert_eq!(plan.state_changes[0].to, TaskState::Completed);
    assert_eq!(plan.operations[0].kind, OperationKind::PropagateTask);
}

#[test]
fn a_rewound_created_child_is_readmitted_not_duplicated() {
    let mut snapshot = snapshot(vec![
        task("step1", 1, TaskState::Running),
        task("b1", 0, TaskState::Cancelled),
        // The Cancel rewind recreated b1 at generation 1; it never ran.
        (
            id("b1", 1),
            TaskSnapshot {
                model_node: node("b1"),
                parent: Some(id("step1", 1)),
                predecessors: vec![id("step1", 1)],
                state: TaskState::Created,
                revision: 0,
            },
        ),
    ]);
    snapshot.version.graph = 2;
    let command = ContainerFanoutCommand {
        process_id: "p1".into(),
        source: id("step1", 1),
        source_operation: OperationCommit {
            id: OperationId {
                process_id: "p1".into(),
                task: id("step1", 1),
                sequence: 4,
            },
            claim: claim(),
        },
        children: vec![child(
            "b1",
            ContainerChildPlan::Entry,
            vec![ContainerPredecessor::Source],
        )],
        inputs: Vec::new(),
        source_park: TaskState::Running,
    };
    let plan = Fanout::plan(&snapshot, &command).unwrap();
    assert!(plan.task_creates.is_empty(), "no duplicate generation");
    // The running source stays running (the park is the state it is
    // already in); the readmitted child is the only change.
    assert_eq!(plan.state_changes.len(), 1);
    assert_eq!(plan.state_changes[0].task, id("b1", 1));
    assert_eq!(plan.state_changes[0].from, TaskState::Created);
    assert_eq!(plan.state_changes[0].to, TaskState::Ready);
    assert_eq!(
        plan.operations[0].id,
        OperationId {
            process_id: "p1".into(),
            task: id("b1", 1),
            sequence: 0,
        }
    );
}

#[test]
fn an_active_child_is_ignored_not_doubled() {
    let snapshot = snapshot(vec![
        task("step1", 0, TaskState::Running),
        (
            id("b1", 0),
            TaskSnapshot {
                model_node: node("b1"),
                parent: Some(id("step1", 0)),
                predecessors: vec![id("step1", 0)],
                state: TaskState::Running,
                revision: 1,
            },
        ),
    ]);
    let command = ContainerFanoutCommand {
        process_id: "p1".into(),
        source: id("step1", 0),
        source_operation: commit("step1", 4),
        children: vec![child(
            "b1",
            ContainerChildPlan::Entry,
            vec![ContainerPredecessor::Source],
        )],
        inputs: Vec::new(),
        source_park: TaskState::Running,
    };
    let plan = Fanout::plan(&snapshot, &command).unwrap();
    assert!(plan.task_creates.is_empty());
    assert!(plan.operations.is_empty());
}

#[test]
fn a_completed_child_reruns_as_a_fresh_generation() {
    let snapshot = snapshot(vec![
        task("step1", 1, TaskState::Running),
        task("b1", 0, TaskState::Completed),
    ]);
    let command = ContainerFanoutCommand {
        process_id: "p1".into(),
        source: id("step1", 1),
        source_operation: OperationCommit {
            id: OperationId {
                process_id: "p1".into(),
                task: id("step1", 1),
                sequence: 2,
            },
            claim: claim(),
        },
        children: vec![child(
            "b1",
            ContainerChildPlan::Entry,
            vec![ContainerPredecessor::Source],
        )],
        inputs: Vec::new(),
        source_park: TaskState::Running,
    };
    let plan = Fanout::plan(&snapshot, &command).unwrap();
    let create = &plan.task_creates[0];
    assert_eq!(create.task, id("b1", 1), "next generation allocated");
}

#[test]
fn a_terminal_source_is_not_a_container() {
    let (snapshot, command) = fanout_command(
        "step1",
        TaskState::Completed,
        vec![child(
            "b1",
            ContainerChildPlan::Entry,
            vec![ContainerPredecessor::Source],
        )],
    );
    let error = Fanout::plan(&snapshot, &command).unwrap_err();
    assert!(matches!(error, ContainerError::SourceNotContainer { .. }));
}

#[test]
fn a_failure_fanout_keeps_the_step_running() {
    let (snapshot, mut command) = fanout_command(
        "step1",
        TaskState::Waiting,
        vec![child(
            "c1",
            ContainerChildPlan::Entry,
            vec![ContainerPredecessor::Source],
        )],
    );
    command.source_park = TaskState::Running;
    let plan = Fanout::plan(&snapshot, &command).unwrap();
    let change = plan
        .state_changes
        .iter()
        .find(|change| change.task == id("step1", 0))
        .expect("the waiting step moves to Running");
    assert_eq!(change.from, TaskState::Waiting);
    assert_eq!(change.to, TaskState::Running);
    assert!(
        plan.state_changes
            .iter()
            .all(|change| { change.to != TaskState::Waiting && change.to != TaskState::Completed })
    );

    // An already-running step keeps its state: the fan-out adds no change.
    let (snapshot, mut command) = fanout_command(
        "step1",
        TaskState::Running,
        vec![child(
            "c1",
            ContainerChildPlan::Entry,
            vec![ContainerPredecessor::Source],
        )],
    );
    command.source_park = TaskState::Running;
    let plan = Fanout::plan(&snapshot, &command).unwrap();
    assert!(
        plan.state_changes
            .iter()
            .all(|change| change.task != id("step1", 0)),
        "a running source has no state change to park it"
    );
}
