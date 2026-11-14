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

/// A client's own `skip` carries its fall-through marker (`CLIENT_SKIP_VAR`)
/// beside its inputs: the propagation reads the execution as *done*, not as a
/// dead path, so the next declared step runs instead of joining a skip
/// cascade. Every other deciding action writes no marker.
#[test]
fn a_client_skip_marks_the_fall_through() {
    let plan =
        ActionPlanner::plan(&snapshot(TaskState::Running), &command(ActionKind::Skip)).unwrap();
    assert_eq!(plan.state_changes[0].to, TaskState::Skipped);
    assert_eq!(
        plan.variable_writes,
        vec![
            VariableWrite {
                scope: task(),
                name: "answer".into(),
                value: "42".into(),
            },
            VariableWrite {
                scope: task(),
                name: crate::scheduler::core::plan::propagation::CLIENT_SKIP_VAR.to_string(),
                value: "true".into(),
            },
        ]
    );

    let complete = ActionPlanner::plan(
        &snapshot(TaskState::Waiting),
        &command(ActionKind::Complete),
    )
    .unwrap();
    assert!(
        complete
            .variable_writes
            .iter()
            .all(|write| write.name != crate::scheduler::core::plan::propagation::CLIENT_SKIP_VAR)
    );
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
/// handler is (the target's own work done, `is_biz_success`). The finished act
/// is the *origin* of the rewind — it keeps its state and a `ReenterTask`
/// re-runs it — so the decision marks no state on it; the work ahead is what
/// the re-entry closes. A cancel whose work is not done
/// (`Ready`/`Running`/`Waiting`) is refused.
#[test]
fn cancel_accepts_only_a_finished_task() {
    for state in [TaskState::Completed, TaskState::Submitted] {
        let plan = ActionPlanner::plan(&snapshot(state), &command(ActionKind::Cancel)).unwrap();
        assert!(
            plan.state_changes.is_empty(),
            "the cancel origin keeps its finished state ({state:?}): {:?}",
            plan.state_changes
        );
    }
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

#[test]
fn an_empty_input_name_is_rejected_before_commit() {
    let mut command = command(ActionKind::Complete);
    command.inputs = vec![("  ".into(), "v".into())];
    assert_eq!(
        ActionPlanner::validate_command(&command),
        Err(CommandError::EmptyInputName)
    );
}

#[test]
fn the_command_envelope_must_match_the_snapshot() {
    let mut wrong_process = command(ActionKind::Complete);
    wrong_process.process_id = "other".into();
    assert!(matches!(
        ActionPlanner::plan(&snapshot(TaskState::Waiting), &wrong_process),
        Err(CommandError::WrongProcess { .. })
    ));

    let mut foreign_source = command(ActionKind::Complete);
    foreign_source.source_operation.id.process_id = "other".into();
    assert!(matches!(
        ActionPlanner::plan(&snapshot(TaskState::Waiting), &foreign_source),
        Err(CommandError::ForeignSourceOperation(_))
    ));

    let mut wrong_target = command(ActionKind::Complete);
    wrong_target.source_operation.id.task = TaskId::new("elsewhere", 0).unwrap();
    assert_eq!(
        ActionPlanner::plan(&snapshot(TaskState::Waiting), &wrong_target),
        Err(CommandError::SourceOperationTargetsDifferentTask)
    );
}

#[test]
fn command_errors_render() {
    assert!(
        CommandError::EmptyInputName
            .to_string()
            .starts_with("command error:")
    );
}
