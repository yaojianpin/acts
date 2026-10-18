use super::*;
use crate::scheduler::core::commit::aggregate::TaskSnapshot;
use crate::scheduler::core::model::model_graph::NodeId;
use crate::scheduler::core::model::operation::ClaimToken;
use crate::scheduler::core::model::transition_plan::ProcessVersion;
use std::collections::{BTreeMap, HashMap};

fn task() -> TaskId {
    TaskId::new("step", 2).unwrap()
}
fn source() -> OperationCommit {
    OperationCommit {
        id: OperationId {
            process_id: "p1".into(),
            task: task(),
            sequence: 4,
        },
        claim: ClaimToken {
            owner: "r1".into(),
            fence: 8,
            lease_until_ms: i64::MAX,
            heartbeat_revision: 0,
        },
    }
}
fn snapshot(state: TaskState) -> ProcessSnapshot {
    ProcessSnapshot {
        process_id: "p1".into(),
        version: ProcessVersion {
            graph: 0,
            process: 3,
            operations: 5,
        },
        tasks: HashMap::from([(
            task(),
            TaskSnapshot {
                model_node: NodeId::parse("step").unwrap(),
                parent: None,
                predecessors: vec![],
                state,
                revision: 7,
            },
        )]),
        variables: BTreeMap::new(),
        operations: HashMap::new(),
        outbox: HashMap::new(),
    }
}
fn command(action: ActionKind) -> ActionCommand {
    ActionCommand {
        process_id: "p1".into(),
        task: task(),
        action,
        inputs: vec![("answer".into(), "42".into())],
        source_operation: source(),
    }
}

#[test]
fn complete_generates_one_atomic_plan() {
    let plan = ActionPlanner::plan(
        &snapshot(TaskState::Waiting),
        &command(ActionKind::Complete),
    )
    .unwrap();
    assert_eq!(plan.state_changes[0].to, TaskState::Completed);
    assert_eq!(plan.variable_writes.len(), 1);
    assert_eq!(plan.operations[0].kind, OperationKind::PropagateTask);
    assert_eq!(plan.source_operation, Some(source()));
}

#[test]
fn terminal_task_rejects_action() {
    assert!(matches!(
        ActionPlanner::plan(&snapshot(TaskState::Completed), &command(ActionKind::Abort),),
        Err(CommandError::ActionNotAllowed { .. })
    ));
    assert!(matches!(
        ActionPlanner::plan(
            &snapshot(TaskState::Backed),
            &command(ActionKind::Fail {
                code: "E".into(),
                message: String::new(),
            }),
        ),
        Err(CommandError::ActionNotAllowed { .. })
    ));
}

/// `Cancel` is the legacy rewind, guard included: accepted where the legacy
/// handler is (the target's own work done, `is_biz_success`), closing that
/// generation `Backed` — the legacy cancelled act's own fact — and refused
/// while the work is not done (`Ready`/`Running`/`Waiting`).
#[test]
fn cancel_accepts_only_a_finished_task() {
    let plan = ActionPlanner::plan(
        &snapshot(TaskState::Completed),
        &command(ActionKind::Cancel),
    )
    .unwrap();
    assert_eq!(plan.state_changes[0].to, TaskState::Backed);
    for state in [TaskState::Ready, TaskState::Running, TaskState::Waiting] {
        assert!(
            matches!(
                ActionPlanner::plan(&snapshot(state), &command(ActionKind::Cancel)),
                Err(CommandError::ActionNotAllowed { .. })
            ),
            "a cancel whose work is not done is refused from {state:?}"
        );
    }
}

#[test]
fn back_closes_current_generation_without_reopening_it() {
    let plan =
        ActionPlanner::plan(&snapshot(TaskState::Waiting), &command(ActionKind::Back)).unwrap();
    assert_eq!(plan.state_changes[0].to, TaskState::Backed);
    // The rewind is a pure hand-off the deciding operation folds into its own
    // commit; a `Back` has no flow to propagate, so no follow-up operation is
    // left in the ready queue.
    assert!(plan.operations.is_empty());
}

#[test]
fn duplicate_inputs_are_rejected_before_commit() {
    let mut command = command(ActionKind::Complete);
    command.inputs.push(("answer".into(), "43".into()));
    assert_eq!(
        ActionPlanner::plan(&snapshot(TaskState::Waiting), &command),
        Err(CommandError::DuplicateInput("answer".into()))
    );
}
#[test]
fn reserved_error_fields_are_rejected_for_every_action() {
    for action in [ActionKind::Complete, ActionKind::Submit, ActionKind::Cancel] {
        let mut command = command(action);
        command.inputs = vec![("error.code".into(), "spoofed".into())];
        assert_eq!(
            ActionPlanner::validate_command(&command),
            Err(CommandError::ReservedErrorInput)
        );
    }
}
