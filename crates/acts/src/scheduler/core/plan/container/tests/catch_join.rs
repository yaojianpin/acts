use super::*;

fn catch_graph() -> ExecutionGraph {
    let mut b = crate::scheduler::core::model::execution_graph::ExecutionGraphBuilder::new("p1", 1)
        .unwrap();
    for task in ["step1", "c1", "c2"] {
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
    b.add_child(id("step1", 0), id("c1", 0)).unwrap();
    b.add_child(id("step1", 0), id("c2", 0)).unwrap();
    b.build().unwrap()
}

/// A failed step deciding its caught error: the step carries the ERROR
/// sign, its CATCH children carry theirs, and the children's states are
/// the argument.
fn catch_snapshot(c1: ExecutionState, c2: ExecutionState) -> AggregateSnapshot {
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
            id("c1", 0),
            ExecutionSnapshot {
                model_node: node("c1"),
                parent: Some(id("step1", 0)),
                predecessors: vec![id("step1", 0)],
                state: c1,
                revision: 1,
            },
        ),
        (
            id("c2", 0),
            ExecutionSnapshot {
                model_node: node("c2"),
                parent: Some(id("step1", 0)),
                predecessors: vec![id("step1", 0)],
                state: c2,
                revision: 1,
            },
        ),
    ]);
    // The failed step carries the deciding flow; its children carry the
    // handler role.
    snapshot.variables.insert(
        (id("step1", 0), FLOW_VAR.to_string()),
        FLOW_CATCHES.to_string(),
    );
    for task in ["c1", "c2"] {
        snapshot
            .variables
            .insert((id(task, 0), SIGN_VAR.to_string()), SIGN_CATCH.to_string());
    }
    snapshot
}

fn catch_command() -> CatchJoinCommand {
    CatchJoinCommand {
        process_id: "p1".into(),
        parent: id("step1", 0),
        source: id("c1", 0),
    }
}

#[test]
fn catch_join_resumes_the_error_when_every_catcher_skipped() {
    let graph = catch_graph();
    let snapshot = catch_snapshot(ExecutionState::Skipped, ExecutionState::Skipped);
    let outcome = ContainerPlanner::plan_catch_join(&graph, &snapshot, &catch_command()).unwrap();
    let Some(CatchJoinOutcome::Resumed(change, operation)) = outcome else {
        panic!("every skipped catcher resumes the error");
    };
    assert_eq!(change.execution, id("step1", 0));
    assert_eq!(change.from, ExecutionState::Running);
    assert_eq!(change.to, ExecutionState::Failed);
    assert_eq!(operation.kind, OperationKind::PropagateTask);
    assert_eq!(operation.id.task_id, "step1");
    assert_eq!(operation.id.sequence, 0);
}

#[test]
fn catch_join_catches_when_a_catcher_did_not_skip() {
    let graph = catch_graph();
    for (c1, c2) in [
        (ExecutionState::Completed, ExecutionState::Skipped),
        (ExecutionState::Completed, ExecutionState::Completed),
        // The legacy `is_empty_catched` only counts skips, so a failed
        // handler also leaves the error caught.
        (ExecutionState::Failed, ExecutionState::Skipped),
    ] {
        let snapshot = catch_snapshot(c1, c2);
        let outcome =
            ContainerPlanner::plan_catch_join(&graph, &snapshot, &catch_command()).unwrap();
        let Some(CatchJoinOutcome::Caught(change, operation)) = outcome else {
            panic!("a non-skipped catcher catches the error ({c1:?}/{c2:?})");
        };
        assert_eq!(change.from, ExecutionState::Running);
        assert_eq!(change.to, ExecutionState::Completed);
        assert_eq!(operation.kind, OperationKind::PropagateTask);
    }
}

#[test]
fn catch_join_waits_for_an_in_flight_catcher_and_yields_to_the_abort_walk() {
    let graph = catch_graph();
    for (c1, c2) in [
        (ExecutionState::Running, ExecutionState::Completed),
        (ExecutionState::Created, ExecutionState::Skipped),
        (ExecutionState::Completed, ExecutionState::Aborted),
    ] {
        let snapshot = catch_snapshot(c1, c2);
        assert!(
            ContainerPlanner::plan_catch_join(&graph, &snapshot, &catch_command())
                .unwrap()
                .is_none(),
            "{c1:?}/{c2:?} must not decide the step"
        );
    }
}

/// A step that declares both `catches` and `timeouts` has a *mixed* child
/// set: the handlers decide the caught error, a timeout branch the tick
/// fired only gates the join (the legacy `auto_complete` waits for every
/// child, `is_empty_catched` counts the CATCH ones), and any child's
/// settlement — the timeout branch's included — may trigger the decision.
#[test]
fn catch_join_decides_a_mixed_child_set_from_its_handlers() {
    let graph = {
        let mut b =
            crate::scheduler::core::model::execution_graph::ExecutionGraphBuilder::new("p1", 1)
                .unwrap();
        for task in ["step1", "c1", "t1"] {
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
        b.add_child(id("step1", 0), id("c1", 0)).unwrap();
        b.add_child(id("step1", 0), id("t1", 0)).unwrap();
        b.build().unwrap()
    };
    let mixed = |handler: ExecutionState, timeout: ExecutionState| {
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
                id("c1", 0),
                ExecutionSnapshot {
                    model_node: node("c1"),
                    parent: Some(id("step1", 0)),
                    predecessors: vec![id("step1", 0)],
                    state: handler,
                    revision: 1,
                },
            ),
            (
                id("t1", 0),
                ExecutionSnapshot {
                    model_node: node("t1"),
                    parent: Some(id("step1", 0)),
                    predecessors: vec![id("step1", 0)],
                    state: timeout,
                    revision: 1,
                },
            ),
        ]);
        snapshot.variables.insert(
            (id("step1", 0), FLOW_VAR.to_string()),
            FLOW_CATCHES.to_string(),
        );
        snapshot
            .variables
            .insert((id("c1", 0), SIGN_VAR.to_string()), SIGN_CATCH.to_string());
        snapshot
    };
    // Triggered by the timeout branch's settlement, not the handler's: the
    // handler caught the error and the branch has settled.
    let caught = ContainerPlanner::plan_catch_join(
        &graph,
        &mixed(ExecutionState::Completed, ExecutionState::Completed),
        &CatchJoinCommand {
            process_id: "p1".into(),
            parent: id("step1", 0),
            source: id("t1", 0),
        },
    )
    .unwrap()
    .expect("the timeout branch's settlement completes the gate");
    assert!(matches!(caught, CatchJoinOutcome::Caught(..)));
    // The handler skipped instead: the error resumes, still triggered by
    // the timeout branch.
    let resumed = ContainerPlanner::plan_catch_join(
        &graph,
        &mixed(ExecutionState::Skipped, ExecutionState::Completed),
        &CatchJoinCommand {
            process_id: "p1".into(),
            parent: id("step1", 0),
            source: id("t1", 0),
        },
    )
    .unwrap()
    .expect("the timeout branch's settlement completes the gate");
    assert!(matches!(resumed, CatchJoinOutcome::Resumed(..)));
    // The timeout branch is still in flight: the gate holds, however the
    // handler settled.
    for handler in [ExecutionState::Completed, ExecutionState::Skipped] {
        assert!(
            ContainerPlanner::plan_catch_join(
                &graph,
                &mixed(handler, ExecutionState::Waiting),
                &CatchJoinCommand {
                    process_id: "p1".into(),
                    parent: id("step1", 0),
                    source: id("t1", 0),
                },
            )
            .unwrap()
            .is_none(),
            "a live timeout branch gates the decision (handler {handler:?})"
        );
    }
    // A foreign execution's settlement triggers nothing.
    assert!(
        ContainerPlanner::plan_catch_join(
            &graph,
            &mixed(ExecutionState::Completed, ExecutionState::Completed),
            &CatchJoinCommand {
                process_id: "p1".into(),
                parent: id("step1", 0),
                source: id("elsewhere", 0),
            },
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn catch_join_only_fires_for_a_flow_decided_step_and_its_catcher() {
    let graph = catch_graph();
    let settled = || catch_snapshot(ExecutionState::Skipped, ExecutionState::Skipped);

    // A step that never failed (no deciding flow) is not deciding anything.
    let mut snapshot = settled();
    snapshot
        .variables
        .remove(&(id("step1", 0), FLOW_VAR.to_string()));
    assert!(
        ContainerPlanner::plan_catch_join(&graph, &snapshot, &catch_command())
            .unwrap()
            .is_none()
    );

    // A step parked for children is a plain container, not a decider: the
    // catch join must not decide it either.
    let mut snapshot = settled();
    snapshot.variables.insert(
        (id("step1", 0), FLOW_VAR.to_string()),
        FLOW_CHILDREN.to_string(),
    );
    assert!(
        ContainerPlanner::plan_catch_join(&graph, &snapshot, &catch_command())
            .unwrap()
            .is_none()
    );

    // The triggering child must belong to the deciding step, but its role
    // no longer has to be a handler one: a step declaring `timeouts` beside
    // its `catches` is decided by whichever child settles last.
    let mut snapshot = settled();
    snapshot
        .variables
        .remove(&(id("c1", 0), SIGN_VAR.to_string()));
    assert!(
        ContainerPlanner::plan_catch_join(&graph, &snapshot, &catch_command())
            .unwrap()
            .is_some(),
        "the handler role must not gate the trigger"
    );

    // A child without the role does not vote: the decision still comes from
    // the handler (c1 skipped, so the error resumes).
    let mut snapshot = settled();
    snapshot
        .variables
        .remove(&(id("c2", 0), SIGN_VAR.to_string()));
    let outcome = ContainerPlanner::plan_catch_join(&graph, &snapshot, &catch_command())
        .unwrap()
        .expect("the signed handler still decides");
    assert!(matches!(outcome, CatchJoinOutcome::Resumed(..)));

    // A decider with no handler child at all is an aggregate the failure
    // fan-out did not produce: refuse to guess a catch outcome for it.
    let mut snapshot = settled();
    for task in ["c1", "c2"] {
        snapshot
            .variables
            .remove(&(id(task, 0), SIGN_VAR.to_string()));
    }
    assert!(
        ContainerPlanner::plan_catch_join(&graph, &snapshot, &catch_command())
            .unwrap()
            .is_none()
    );
}
