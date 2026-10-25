use super::*;

/// Mark one execution as a container parked for its children: the flow
/// the plain join reads (see [`FLOW_CHILDREN`]).
fn mark_parked(snapshot: &mut ProcessSnapshot, parent: &str) {
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
    let mut b = crate::scheduler::core::model::task_graph::TaskGraphBuilder::new("p1", 1).unwrap();
    for (task, generation) in [("step1", 0u64), ("b1", 0), ("b2", 0)] {
        b.add_node(crate::scheduler::core::model::task_graph::TaskNode {
            id: id(task, generation),
            model_node: node(task),
            state: TaskState::Created,
            revision: 0,
        })
        .unwrap();
    }
    b.set_root(id("step1", 0)).unwrap();
    b.add_child(id("step1", 0), id("b1", 0)).unwrap();
    b.add_child(id("step1", 0), id("b2", 0)).unwrap();
    let graph = b.build().unwrap();
    let mut snapshot = snapshot(vec![
        (
            id("step1", 0),
            TaskSnapshot {
                model_node: node("step1"),
                parent: None,
                predecessors: vec![],
                state: TaskState::Running,
                revision: 3,
            },
        ),
        (
            id("b1", 0),
            TaskSnapshot {
                model_node: node("b1"),
                parent: Some(id("step1", 0)),
                predecessors: vec![id("step1", 0)],
                state: TaskState::Completed,
                revision: 5,
            },
        ),
        (
            id("b2", 0),
            TaskSnapshot {
                model_node: node("b2"),
                parent: Some(id("step1", 0)),
                predecessors: vec![id("step1", 0)],
                state: TaskState::Skipped,
                revision: 4,
            },
        ),
    ]);
    mark_parked(&mut snapshot, "step1");
    let command = ContainerJoinCommand {
        process_id: "p1".into(),
        parent: id("step1", 0),
        join: None,
    };
    let outcome = Join::plan((&graph, &snapshot), &command).unwrap().unwrap();
    let change = outcome.change;
    let operation = outcome.operation;
    assert!(outcome.cancels.is_empty());
    assert!(outcome.failure.is_none());
    assert_eq!(change.task, id("step1", 0));
    assert_eq!(change.from, TaskState::Running);
    assert_eq!(change.to, TaskState::Completed);
    assert_eq!(operation.kind, OperationKind::PropagateTask);
    assert_eq!(operation.id.task.node.as_str(), "step1");
    assert_eq!(operation.id.sequence, 1);
}

#[test]
fn join_waits_for_running_children_and_never_fires_on_an_aborted_one() {
    let graph = {
        let mut b =
            crate::scheduler::core::model::task_graph::TaskGraphBuilder::new("p1", 1).unwrap();
        for task in ["step1", "b1", "b2"] {
            b.add_node(crate::scheduler::core::model::task_graph::TaskNode {
                id: id(task, 0),
                model_node: node(task),
                state: TaskState::Created,
                revision: 0,
            })
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
        join: None,
    };
    // The parent is a running container carrying the park mark: that is
    // the state a child's settlement decides.
    let parked_child = |b1: TaskState, b2: TaskState| {
        let mut snapshot = snapshot(vec![
            (
                id("step1", 0),
                TaskSnapshot {
                    model_node: node("step1"),
                    parent: None,
                    predecessors: vec![],
                    state: TaskState::Running,
                    revision: 3,
                },
            ),
            (
                id("b1", 0),
                TaskSnapshot {
                    model_node: node("b1"),
                    parent: Some(id("step1", 0)),
                    predecessors: vec![id("step1", 0)],
                    state: b1,
                    revision: 1,
                },
            ),
            (
                id("b2", 0),
                TaskSnapshot {
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
        Join::plan(
            (
                &graph,
                &parked_child(TaskState::Running, TaskState::Completed)
            ),
            &command,
        )
        .unwrap()
        .is_none()
    );
    // An aborted child leaves the outcome to the abort walk.
    assert!(
        Join::plan(
            (
                &graph,
                &parked_child(TaskState::Completed, TaskState::Aborted)
            ),
            &command,
        )
        .unwrap()
        .is_none()
    );
    // A settled child set under a parent nobody marked decides nothing:
    // an execution waiting for its own act (the legacy `Interrupt`) is
    // `Waiting`, and one whose act is in flight is `Running` — neither is
    // a container until its flow says so.
    let mut unmarked = parked_child(TaskState::Completed, TaskState::Skipped);
    unmarked.variables.clear();
    assert!(Join::plan((&graph, &unmarked), &command).unwrap().is_none());
    // A parent deciding its catches is never joined by the plain join
    // either — that flow belongs to the catch join.
    let mut decider = parked_child(TaskState::Completed, TaskState::Skipped);
    decider.variables.insert(
        (id("step1", 0), FLOW_VAR.to_string()),
        FLOW_CATCHES.to_string(),
    );
    assert!(Join::plan((&graph, &decider), &command).unwrap().is_none());
    // A `Waiting` parent is never joined either: its act is unanswered.
    let mut waiting = parked_child(TaskState::Completed, TaskState::Skipped);
    waiting.tasks.insert(
        id("step1", 0),
        TaskSnapshot {
            model_node: node("step1"),
            parent: None,
            predecessors: vec![],
            state: TaskState::Waiting,
            revision: 3,
        },
    );
    assert!(Join::plan((&graph, &waiting), &command).unwrap().is_none());
}

#[test]
fn join_refuses_a_foreign_process_or_stale_graph() {
    let command = ContainerJoinCommand {
        process_id: "other".into(),
        parent: id("step1", 0),
        join: None,
    };
    let graph = {
        let mut b =
            crate::scheduler::core::model::task_graph::TaskGraphBuilder::new("p1", 1).unwrap();
        b.add_node(crate::scheduler::core::model::task_graph::TaskNode {
            id: id("step1", 0),
            model_node: node("step1"),
            state: TaskState::Created,
            revision: 0,
        })
        .unwrap();
        b.set_root(id("step1", 0)).unwrap();
        b.build().unwrap()
    };
    let snapshot = snapshot(vec![task("step1", 0, TaskState::Running)]);
    assert_eq!(
        Join::plan((&graph, &snapshot), &command),
        Err(ContainerError::WrongProcess)
    );

    let mut stale = snapshot.clone();
    stale.version.graph = 9;
    assert_eq!(
        Join::plan(
            (&graph, &stale,),
            &ContainerJoinCommand {
                process_id: "p1".into(),
                parent: id("step1", 0),
                join: None,
            },
        ),
        Err(ContainerError::GraphVersionConflict {
            graph: 1,
            snapshot: 9
        })
    );
}

#[test]
fn a_container_without_children_never_joins() {
    let graph = {
        let mut b =
            crate::scheduler::core::model::task_graph::TaskGraphBuilder::new("p1", 1).unwrap();
        b.add_node(crate::scheduler::core::model::task_graph::TaskNode {
            id: id("step1", 0),
            model_node: node("step1"),
            state: TaskState::Created,
            revision: 0,
        })
        .unwrap();
        b.set_root(id("step1", 0)).unwrap();
        b.build().unwrap()
    };
    let mut snapshot = snapshot(vec![task("step1", 0, TaskState::Running)]);
    mark_parked(&mut snapshot, "step1");
    assert!(
        Join::plan(
            (&graph, &snapshot),
            &ContainerJoinCommand {
                process_id: "p1".into(),
                parent: id("step1", 0),
                join: None,
            }
        )
        .unwrap()
        .is_none()
    );
}

/// A parked container over `children` — `(node, state)` pairs — with the
/// rule the join decides by.
fn join_container(
    join: Option<crate::JoinRule>,
    children: &[(&str, TaskState)],
) -> (
    crate::scheduler::core::model::task_graph::TaskGraph,
    ProcessSnapshot,
    ContainerJoinCommand,
) {
    let mut b = crate::scheduler::core::model::task_graph::TaskGraphBuilder::new("p1", 1).unwrap();
    b.add_node(crate::scheduler::core::model::task_graph::TaskNode {
        id: id("step1", 0),
        model_node: node("step1"),
        state: TaskState::Running,
        revision: 2,
    })
    .unwrap();
    let mut tasks = vec![task("step1", 0, TaskState::Running)];
    for (name, state) in children {
        b.add_node(crate::scheduler::core::model::task_graph::TaskNode {
            id: id(name, 0),
            model_node: node(name),
            state: TaskState::Ready,
            revision: 2,
        })
        .unwrap();
        b.add_child(id("step1", 0), id(name, 0)).unwrap();
        tasks.push(task(name, 0, *state));
    }
    b.set_root(id("step1", 0)).unwrap();
    let graph = b.build().unwrap();
    let mut snapshot = snapshot(tasks);
    mark_parked(&mut snapshot, "step1");
    let command = ContainerJoinCommand {
        process_id: "p1".into(),
        parent: id("step1", 0),
        join,
    };
    (graph, snapshot, command)
}

#[test]
fn any_rule_completes_on_the_first_completion_and_withdraws_the_stragglers() {
    let (graph, snapshot, command) = join_container(
        Some(crate::JoinRule::Any),
        &[
            ("b1", TaskState::Completed),
            ("b2", TaskState::Running),
            ("b3", TaskState::Waiting),
        ],
    );
    let outcome = Join::plan((&graph, &snapshot), &command).unwrap().unwrap();
    assert_eq!(outcome.change.to, TaskState::Completed);
    assert_eq!(outcome.cancels.len(), 2, "both in-flight votes withdraw");
    assert!(
        outcome
            .cancels
            .iter()
            .all(|intent| intent.kind == OperationKind::SkipTask)
    );
    assert!(outcome.failure.is_none());
}

#[test]
fn any_rule_waits_while_a_completion_is_still_possible() {
    let (graph, snapshot, command) = join_container(
        Some(crate::JoinRule::Any),
        &[("b1", TaskState::Failed), ("b2", TaskState::Running)],
    );
    assert!(Join::plan((&graph, &snapshot), &command).unwrap().is_none());
}

#[test]
fn any_rule_fails_when_every_child_settled_without_a_completion() {
    let (graph, snapshot, command) = join_container(
        Some(crate::JoinRule::Any),
        &[("b1", TaskState::Failed), ("b2", TaskState::Failed)],
    );
    let outcome = Join::plan((&graph, &snapshot), &command).unwrap().unwrap();
    assert_eq!(outcome.change.to, TaskState::Failed);
    let (code, message) = outcome.failure.expect("the failure names the rule");
    assert_eq!(code, "join_failed");
    assert!(message.contains("any"), "{message}");
    // A failed container's propagation is a dead-source cascade: the
    // stragglers are its business, not the join's.
    assert!(outcome.cancels.is_empty());
}

#[test]
fn partial_count_completes_on_the_threshold_and_fails_beyond_reach() {
    // one completion of the two the rule needs: the join waits
    let (graph, snapshot, command) = join_container(
        Some(crate::JoinRule::Partial(crate::PartialThreshold::Count(2))),
        &[
            ("b1", TaskState::Completed),
            ("b2", TaskState::Running),
            ("b3", TaskState::Running),
        ],
    );
    assert!(Join::plan((&graph, &snapshot), &command).unwrap().is_none());

    // the second completion meets the threshold: the container completes and
    // the last vote is withdrawn
    let (graph, snapshot, command) = join_container(
        Some(crate::JoinRule::Partial(crate::PartialThreshold::Count(2))),
        &[
            ("b1", TaskState::Completed),
            ("b2", TaskState::Completed),
            ("b3", TaskState::Running),
        ],
    );
    let outcome = Join::plan((&graph, &snapshot), &command).unwrap().unwrap();
    assert_eq!(outcome.change.to, TaskState::Completed);
    assert_eq!(outcome.cancels.len(), 1);

    // one child left can reach one completion of the two the rule needs
    let (graph, snapshot, command) = join_container(
        Some(crate::JoinRule::Partial(crate::PartialThreshold::Count(2))),
        &[
            ("b1", TaskState::Failed),
            ("b2", TaskState::Failed),
            ("b3", TaskState::Running),
        ],
    );
    let outcome = Join::plan((&graph, &snapshot), &command).unwrap().unwrap();
    assert_eq!(outcome.change.to, TaskState::Failed);
}

#[test]
fn partial_percent_counts_only_the_children_in_the_vote() {
    // 50% of four is two
    let (graph, snapshot, command) = join_container(
        Some(crate::JoinRule::Partial(crate::PartialThreshold::Percent(
            50,
        ))),
        &[
            ("b1", TaskState::Completed),
            ("b2", TaskState::Completed),
            ("b3", TaskState::Running),
            ("b4", TaskState::Running),
        ],
    );
    let outcome = Join::plan((&graph, &snapshot), &command).unwrap().unwrap();
    assert_eq!(outcome.change.to, TaskState::Completed);
    assert_eq!(outcome.cancels.len(), 2);

    // a branch its own guard skipped was never in the vote: 40% of one
    // counted child is one completion
    let (graph, snapshot, command) = join_container(
        Some(crate::JoinRule::Partial(crate::PartialThreshold::Percent(
            40,
        ))),
        &[("b1", TaskState::Skipped), ("b2", TaskState::Completed)],
    );
    let outcome = Join::plan((&graph, &snapshot), &command).unwrap().unwrap();
    assert_eq!(outcome.change.to, TaskState::Completed);
    assert!(outcome.cancels.is_empty());
}

#[test]
fn an_aborted_child_still_yields_the_rule_join_to_the_abort_walk() {
    let (graph, snapshot, command) = join_container(
        Some(crate::JoinRule::Any),
        &[("b1", TaskState::Completed), ("b2", TaskState::Aborted)],
    );
    assert!(Join::plan((&graph, &snapshot), &command).unwrap().is_none());
}

#[test]
fn a_delegated_rule_never_decides_by_count() {
    let (graph, snapshot, command) = join_container(
        Some(crate::JoinRule::Action {
            action: "sign".into(),
            params: serde_json::Value::Null,
        }),
        &[("b1", TaskState::Completed), ("b2", TaskState::Completed)],
    );
    // the verdicts decide it, in the shell
    assert!(Join::plan((&graph, &snapshot), &command).unwrap().is_none());
}
