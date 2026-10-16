use super::*;
use crate::scheduler::core::commit::aggregate::TaskSnapshot;
use crate::scheduler::core::model::operation::OperationId;
use crate::scheduler::core::model::transition_plan::ProcessVersion;
use crate::scheduler::core::model::transition_plan::TransitionPlan;
use std::collections::{BTreeMap, HashMap};

fn id(task: &str, generation: u64) -> TaskId {
    TaskId::new(task, generation).unwrap()
}

fn node(name: &str) -> NodeId {
    NodeId::parse(name).unwrap()
}

fn commit(task: &str, sequence: u64) -> OperationCommit {
    OperationCommit {
        id: OperationId {
            process_id: "p1".into(),
            node: task.into(),
            generation: 0,
            sequence,
        },
        claim: crate::scheduler::core::model::operation::ClaimToken {
            owner: "r1".into(),
            fence: 1,
            lease_until_ms: i64::MAX,
            heartbeat_revision: 0,
        },
    }
}

fn branch(task: &str) -> TimeoutBranch {
    TimeoutBranch {
        node: task.into(),
        model_node: node(task),
        vars: Vec::new(),
        auto: true,
    }
}

/// A snapshot with `root` (Running) and one owner execution `s1` (Waiting),
/// plus whatever the test adds.
fn snapshot(extra: Vec<(TaskId, TaskSnapshot)>) -> ProcessSnapshot {
    let root = id("$", 0);
    let owner = id("s1", 0);
    let mut tasks = HashMap::from_iter([
        (
            root.clone(),
            TaskSnapshot {
                model_node: node("$"),
                parent: None,
                predecessors: Vec::new(),
                state: crate::scheduler::core::model::task_graph::TaskState::Running,
                revision: 1,
            },
        ),
        (
            owner.clone(),
            TaskSnapshot {
                model_node: node("s1"),
                parent: Some(root),
                predecessors: Vec::new(),
                state: crate::scheduler::core::model::task_graph::TaskState::Waiting,
                revision: 1,
            },
        ),
    ]);
    for (id, task) in extra {
        tasks.insert(id, task);
    }
    ProcessSnapshot {
        process_id: "p1".into(),
        version: ProcessVersion {
            graph: 1,
            process: 2,
            operations: 2,
        },
        tasks,
        variables: BTreeMap::new(),
        operations: HashMap::new(),
        outbox: HashMap::new(),
    }
}

fn child_of(
    owner: &TaskId,
    task: &str,
    state: crate::scheduler::core::model::task_graph::TaskState,
) -> (TaskId, TaskSnapshot) {
    (
        id(task, 0),
        TaskSnapshot {
            model_node: node(task),
            parent: Some(owner.clone()),
            predecessors: vec![owner.clone()],
            state,
            revision: 0,
        },
    )
}

#[test]
fn the_candidate_is_the_first_branch_that_neither_fired_nor_is_in_flight() {
    let owner = id("s1", 0);
    let branches = vec![branch("t1"), branch("t2")];
    let mut snapshot = snapshot(Vec::new());
    assert_eq!(
        candidate(&snapshot, &owner, &branches),
        TimeoutCandidate::Branch(0)
    );
    // t1 fired: the round moves to t2.
    snapshot.variables.insert(
        (owner.clone(), consts::TASK_TIMEOUTS.to_string()),
        r#"["t1"]"#.to_string(),
    );
    assert_eq!(
        candidate(&snapshot, &owner, &branches),
        TimeoutCandidate::Branch(1)
    );
    // t2 fired too: nothing left.
    snapshot.variables.insert(
        (owner.clone(), consts::TASK_TIMEOUTS.to_string()),
        r#"["t1","t2"]"#.to_string(),
    );
    assert_eq!(
        candidate(&snapshot, &owner, &branches),
        TimeoutCandidate::None
    );
}

#[test]
fn an_in_flight_branch_stops_the_round_instead_of_moving_on() {
    let owner = id("s1", 0);
    let branches = vec![branch("t1"), branch("t2")];
    // t1 in flight (unclaimed — a raced replay or a crash cut): the round
    // must not run ahead to t2.
    let in_flight = snapshot(vec![child_of(
        &owner,
        "t1",
        crate::scheduler::core::model::task_graph::TaskState::Running,
    )]);
    assert_eq!(
        candidate(&in_flight, &owner, &branches),
        TimeoutCandidate::Blocked
    );
    // A terminal t1 does not block; it only leaves the candidate at t2 when
    // its marker was taken, and at t1 itself when it was not.
    let settled = snapshot(vec![child_of(
        &owner,
        "t1",
        crate::scheduler::core::model::task_graph::TaskState::Completed,
    )]);
    assert_eq!(
        candidate(&settled, &owner, &branches),
        TimeoutCandidate::Branch(0)
    );
}

#[test]
fn the_cost_counts_from_the_start_stamp_and_never_goes_negative() {
    assert_eq!(cost_ms(1_500, Some("1000")), 500);
    // A missing stamp (an aggregate seeded before this fact) counts as
    // "started now", so a guard cannot fire on an unmeasurable age.
    assert_eq!(cost_ms(1_500, None), 0);
    // A stamp in the future (a clock that stepped back) is not an age.
    assert_eq!(cost_ms(1_000, Some("1500")), 0);
    assert_eq!(cost_ms(1_500, Some("not a number")), 0);
}

#[test]
fn the_claim_appends_one_branch_and_never_repeats_it() {
    assert_eq!(claim(&[], "t1"), vec!["t1".to_string()]);
    assert_eq!(
        claim(&["t1".to_string()], "t2"),
        vec!["t1".to_string(), "t2".to_string()]
    );
    assert_eq!(claim(&["t1".to_string()], "t1"), vec!["t1".to_string()]);
    // The marker is the legacy task datum, not a new key.
    assert_eq!(consts::TASK_TIMEOUTS, "__timeouts");
    assert_eq!(consts::TASK_COST, "__cost");
}

#[test]
fn only_the_declared_branches_keep_the_owner_from_completing() {
    let owner = id("s1", 0);
    let branches = vec![branch("t1")];
    assert!(!has_live_children(&snapshot(Vec::new()), &owner, &branches));
    let live = snapshot(vec![child_of(
        &owner,
        "t1",
        crate::scheduler::core::model::task_graph::TaskState::Ready,
    )]);
    assert!(has_live_children(&live, &owner, &branches));
    // A settled branch does not hold the owner back.
    let settled = snapshot(vec![child_of(
        &owner,
        "t1",
        crate::scheduler::core::model::task_graph::TaskState::Skipped,
    )]);
    assert!(!has_live_children(&settled, &owner, &branches));
    // A child the model never declared as a timeout branch (a branch of a
    // branched step) is not this fact's business.
    let other = snapshot(vec![child_of(
        &owner,
        "b1",
        crate::scheduler::core::model::task_graph::TaskState::Running,
    )]);
    assert!(!has_live_children(&other, &owner, &branches));
}

#[test]
fn the_dispatch_admits_one_entry_child_and_freezes_the_cost_and_marker() {
    let owner = id("s1", 0);
    let snapshot = snapshot(Vec::new());
    let dispatch = dispatch(
        &snapshot,
        &owner,
        commit("s1", 3),
        &branch("t1"),
        vec![(
            "__branch_error".into(),
            r#"{"code":"","message":"x"}"#.into(),
        )],
        &["t1".to_string()],
        1_617,
    );
    let command = &dispatch.command;
    assert_eq!(command.source, owner);
    assert_eq!(command.children.len(), 1);
    let child = &command.children[0];
    assert_eq!(child.node, "t1");
    assert_eq!(child.plan, ContainerChildPlan::Entry);
    assert_eq!(child.predecessors, vec![ContainerPredecessor::Source]);
    assert!(child.auto);
    assert_eq!(
        child.vars,
        vec![(
            "__branch_error".to_string(),
            r#"{"code":"","message":"x"}"#.to_string()
        )]
    );
    assert_eq!(
        command.inputs,
        vec![
            ("__cost".to_string(), "1617".to_string()),
            ("__timeouts".to_string(), r#"["t1"]"#.to_string()),
        ]
    );
    // The owner keeps the state it had: a timed-out execution is not parked
    // by a dispatch.
    assert_eq!(
        command.source_park,
        crate::scheduler::core::model::task_graph::TaskState::Waiting
    );
    // The plan the container machinery builds from it is legal.
    let plan =
        crate::scheduler::core::plan::container::ContainerPlanner::plan_fanout(&snapshot, command)
            .unwrap();
    plan.validate().unwrap();
    assert_eq!(plan.task_creates.len(), 1);
    assert_eq!(
        plan.state_changes,
        Vec::new(),
        "a dispatch never moves the owner"
    );
}

#[test]
fn the_dispatch_reports_the_marker_it_leaves_behind() {
    let owner = id("s1", 0);
    let snapshot = snapshot(Vec::new());
    // The caller re-arms its timer while the marker is shorter than the
    // declaration: one branch claimed of two declared re-arms, the last one
    // retires the timer.
    let one = dispatch(
        &snapshot,
        &owner,
        commit("s1", 3),
        &branch("t1"),
        Vec::new(),
        &["t1".to_string()],
        10,
    );
    assert_eq!(one.claimed_after, vec!["t1".to_string()]);
    assert!(one.claimed_after.len() < 2);
    let both = dispatch(
        &snapshot,
        &owner,
        commit("s1", 4),
        &branch("t2"),
        Vec::new(),
        &["t1".to_string(), "t2".to_string()],
        10,
    );
    assert!(!(both.claimed_after.len() < 2));
}

#[test]
fn the_start_write_stamps_the_tasks_own_scope() {
    let write = start_write(&id("s1", 0), 42);
    assert_eq!(write.name, "__started");
    assert_eq!(write.value, "42");
    assert_eq!(write.scope, id("s1", 0));
}

#[test]
fn the_dispatch_command_plans_through_the_container_machinery() {
    // Guard against the dispatch drifting away from the container plan
    // shape: the command is a `ContainerFanoutCommand`, the plan a
    // `TransitionPlan` built by the same planner every other fan-out uses.
    let snapshot = snapshot(Vec::new());
    let command: ContainerFanoutCommand = dispatch(
        &snapshot,
        &id("s1", 0),
        commit("s1", 1),
        &branch("t1"),
        Vec::new(),
        &["t1".to_string()],
        0,
    )
    .command;
    let plan: TransitionPlan =
        crate::scheduler::core::plan::container::ContainerPlanner::plan_fanout(&snapshot, &command)
            .unwrap();
    assert_eq!(plan.process_id, "p1");
}
