use super::*;

/// Mark one execution as a container parked for its children: the flow
/// the plain join reads (see [`FLOW_CHILDREN`]).
fn mark_parked(snapshot: &mut AggregateSnapshot, parent: &str) {
    snapshot.variables.insert(
        (id(parent, 0), FLOW_VAR.to_string()),
        FLOW_CHILDREN.to_string(),
    );
}

/// The flow fact is one axis with two values, and it is what tells the two
/// joins apart: a `Running` execution with children decides by its flow and
/// by nothing else. An execution in its own act's flow carries no flow fact

#[test]
fn join_completes_the_parent_when_every_child_is_terminal() {
    let mut b = crate::scheduler::core::model::execution_graph::ExecutionGraphBuilder::new("p1", 1)
        .unwrap();
    for (task, generation) in [("step1", 0u64), ("b1", 0), ("b2", 0)] {
        b.add_node(
            crate::scheduler::core::model::execution_graph::ExecutionNode {
                id: id(task, generation),
                model_node: node(task),
                state: ExecutionState::Created,
                revision: 0,
            },
        )
        .unwrap();
    }
    b.set_root(id("step1", 0)).unwrap();
    b.add_child(id("step1", 0), id("b1", 0)).unwrap();
    b.add_child(id("step1", 0), id("b2", 0)).unwrap();
    let graph = b.build().unwrap();
    let mut snapshot = snapshot(vec![
        (
            id("step1", 0),
            ExecutionSnapshot {
                model_node: node("step1"),
                parent: None,
                predecessors: vec![],
                state: ExecutionState::Running,
                revision: 3,
            },
        ),
        (
            id("b1", 0),
            ExecutionSnapshot {
                model_node: node("b1"),
                parent: Some(id("step1", 0)),
                predecessors: vec![id("step1", 0)],
                state: ExecutionState::Completed,
                revision: 5,
            },
        ),
        (
            id("b2", 0),
            ExecutionSnapshot {
                model_node: node("b2"),
                parent: Some(id("step1", 0)),
                predecessors: vec![id("step1", 0)],
                state: ExecutionState::Skipped,
                revision: 4,
            },
        ),
    ]);
    mark_parked(&mut snapshot, "step1");
    let command = ContainerJoinCommand {
        process_id: "p1".into(),
        parent: id("step1", 0),
    };
    let (change, operation) = ContainerPlanner::plan_join(&graph, &snapshot, &command)
        .unwrap()
        .unwrap();
    assert_eq!(change.execution, id("step1", 0));
    assert_eq!(change.from, ExecutionState::Running);
    assert_eq!(change.to, ExecutionState::Completed);
    assert_eq!(operation.kind, OperationKind::PropagateTask);
    assert_eq!(operation.id.task_id, "step1");
    assert_eq!(operation.id.sequence, 1);
}

#[test]
fn join_waits_for_running_children_and_never_fires_on_an_aborted_one() {
    let graph = {
        let mut b =
            crate::scheduler::core::model::execution_graph::ExecutionGraphBuilder::new("p1", 1)
                .unwrap();
        for task in ["step1", "b1", "b2"] {
            b.add_node(
                crate::scheduler::core::model::execution_graph::ExecutionNode {
                    id: id(task, 0),
                    model_node: node(task),
                    state: ExecutionState::Created,
                    revision: 0,
                },
            )
            .unwrap();
        }
        b.set_root(id("step1", 0)).unwrap();
        b.add_child(id("step1", 0), id("b1", 0)).unwrap();
        b.add_child(id("step1", 0), id("b2", 0)).unwrap();
        b.build().unwrap()
    };
    let command = ContainerJoinCommand {
        process_id: "p1".into(),
        parent: id("step1", 0),
    };
    // The parent is a running container carrying the park mark: that is
    // the state a child's settlement decides.
    let parked_child = |b1: ExecutionState, b2: ExecutionState| {
        let mut snapshot = snapshot(vec![
            (
                id("step1", 0),
                ExecutionSnapshot {
                    model_node: node("step1"),
                    parent: None,
                    predecessors: vec![],
                    state: ExecutionState::Running,
                    revision: 3,
                },
            ),
            (
                id("b1", 0),
                ExecutionSnapshot {
                    model_node: node("b1"),
                    parent: Some(id("step1", 0)),
                    predecessors: vec![id("step1", 0)],
                    state: b1,
                    revision: 1,
                },
            ),
            (
                id("b2", 0),
                ExecutionSnapshot {
                    model_node: node("b2"),
                    parent: Some(id("step1", 0)),
                    predecessors: vec![id("step1", 0)],
                    state: b2,
                    revision: 1,
                },
            ),
        ]);
        mark_parked(&mut snapshot, "step1");
        snapshot
    };
    assert!(
        ContainerPlanner::plan_join(
            &graph,
            &parked_child(ExecutionState::Running, ExecutionState::Completed),
            &command
        )
        .unwrap()
        .is_none()
    );
    // An aborted child leaves the outcome to the abort walk.
    assert!(
        ContainerPlanner::plan_join(
            &graph,
            &parked_child(ExecutionState::Completed, ExecutionState::Aborted),
            &command
        )
        .unwrap()
        .is_none()
    );
    // A settled child set under a parent nobody marked decides nothing:
    // an execution waiting for its own act (the legacy `Interrupt`) is
    // `Waiting`, and one whose act is in flight is `Running` — neither is
    // a container until its flow says so.
    let mut unmarked = parked_child(ExecutionState::Completed, ExecutionState::Skipped);
    unmarked.variables.clear();
    assert!(
        ContainerPlanner::plan_join(&graph, &unmarked, &command)
            .unwrap()
            .is_none()
    );
    // A parent deciding its catches is never joined by the plain join
    // either — that flow belongs to the catch join.
    let mut decider = parked_child(ExecutionState::Completed, ExecutionState::Skipped);
    decider.variables.insert(
        (id("step1", 0), FLOW_VAR.to_string()),
        FLOW_CATCHES.to_string(),
    );
    assert!(
        ContainerPlanner::plan_join(&graph, &decider, &command)
            .unwrap()
            .is_none()
    );
    // A `Waiting` parent is never joined either: its act is unanswered.
    let mut waiting = parked_child(ExecutionState::Completed, ExecutionState::Skipped);
    waiting.executions.insert(
        id("step1", 0),
        ExecutionSnapshot {
            model_node: node("step1"),
            parent: None,
            predecessors: vec![],
            state: ExecutionState::Waiting,
            revision: 3,
        },
    );
    assert!(
        ContainerPlanner::plan_join(&graph, &waiting, &command)
            .unwrap()
            .is_none()
    );
}
