use crate::scheduler::core::{
    commit::process::ProcessSnapshot,
    model::{
        operation::{OperationId, OperationKind},
        task_graph::{TaskId, TaskState},
        transition_plan::{
            OperationCommit, OperationIntent, OutboxIntent, StateChange, TransitionPlan,
            VariableWrite,
        },
    },
};

use std::collections::HashSet;
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ActionKind {
    Complete,
    Submit,
    Skip,
    Cancel,
    Abort,
    Back,
    Fail { code: String, message: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ApplyActionCommand {
    pub(crate) process_id: String,
    pub(crate) task: TaskId,
    pub(crate) action: ActionKind,
    pub(crate) inputs: Vec<(String, String)>,
    pub(crate) source_operation: OperationCommit,
}

pub(crate) struct ActionPlanner;

impl ActionPlanner {
    /// Validate invariants that depend only on the command envelope. This runs
    /// before durable ownership is acquired, so caller-correctable failures do
    /// not consume or quarantine an operation.
    pub(crate) fn validate_command(command: &ApplyActionCommand) -> Result<(), CommandError> {
        variable_writes(command)?;
        command
            .source_operation
            .id
            .sequence
            .checked_add(1)
            .ok_or(CommandError::SequenceOverflow)?;
        Ok(())
    }

    pub(crate) fn plan(
        snapshot: &ProcessSnapshot,
        command: &ApplyActionCommand,
    ) -> Result<TransitionPlan, CommandError> {
        Self::validate_command(command)?;
        validate_envelope(snapshot, command)?;
        let current = snapshot
            .tasks
            .get(&command.task)
            .ok_or_else(|| CommandError::TaskNotFound(command.task.clone()))?;
        let target = action_target(current.state, &command.action)?;
        let writes = variable_writes(command)?;
        let next_sequence = command
            .source_operation
            .id
            .sequence
            .checked_add(1)
            .ok_or(CommandError::SequenceOverflow)?;
        let operation_kind = if matches!(command.action, ActionKind::Back) {
            OperationKind::ApplyAction
        } else {
            OperationKind::PropagateTask
        };
        let plan = TransitionPlan {
            process_id: command.process_id.clone(),
            expected: snapshot.version.clone(),
            task_creates: Vec::new(),
            state_changes: vec![StateChange {
                task: command.task.clone(),
                expected_revision: current.revision,
                from: current.state,
                to: target,
            }],
            variable_writes: writes,
            source_operation: Some(command.source_operation.clone()),
            operations: vec![OperationIntent {
                id: OperationId {
                    process_id: command.process_id.clone(),
                    node: command.task.node.clone(),
                    generation: command.task.generation,
                    sequence: next_sequence,
                },
                kind: operation_kind,
            }],
            outbox: vec![OutboxIntent {
                key: format!(
                    "action:{}:{}:{}:{}",
                    command.process_id,
                    command.task.node,
                    command.task.generation,
                    command.source_operation.id.sequence
                ),
                topic: format!("task.{}", action_name(&command.action)),
                payload: format!("{}:{}", command.task.node, command.task.generation),
                receipt: None,
            }],
        };
        plan.validate()
            .map_err(|error| CommandError::InvalidPlan(error.to_string()))?;
        Ok(plan)
    }
}

fn validate_envelope(
    snapshot: &ProcessSnapshot,
    command: &ApplyActionCommand,
) -> Result<(), CommandError> {
    if snapshot.process_id != command.process_id {
        return Err(CommandError::WrongProcess {
            expected: snapshot.process_id.clone(),
            actual: command.process_id.clone(),
        });
    }
    let source = &command.source_operation.id;
    if source.process_id != command.process_id {
        return Err(CommandError::ForeignSourceOperation(source.clone()));
    }
    if source.node != command.task.node || source.generation != command.task.generation {
        return Err(CommandError::SourceOperationTargetsDifferentTask);
    }
    Ok(())
}

fn variable_writes(command: &ApplyActionCommand) -> Result<Vec<VariableWrite>, CommandError> {
    let mut names = HashSet::new();
    let mut writes = Vec::with_capacity(command.inputs.len() + 2);
    for (name, value) in &command.inputs {
        if name.trim().is_empty() {
            return Err(CommandError::EmptyInputName);
        }
        if is_reserved_error_input(name) {
            return Err(CommandError::ReservedErrorInput);
        }
        if !names.insert(name.clone()) {
            return Err(CommandError::DuplicateInput(name.clone()));
        }
        writes.push(VariableWrite {
            scope: command.task.clone(),
            name: name.clone(),
            value: value.clone(),
        });
    }
    if let ActionKind::Fail { code, message } = &command.action {
        writes.push(VariableWrite {
            scope: command.task.clone(),
            name: "error.code".into(),
            value: code.clone(),
        });
        writes.push(VariableWrite {
            scope: command.task.clone(),
            name: "error.message".into(),
            value: message.clone(),
        });
    }
    Ok(writes)
}

fn is_reserved_error_input(name: &str) -> bool {
    matches!(name, "error.code" | "error.message")
}

fn action_target(current: TaskState, action: &ActionKind) -> Result<TaskState, CommandError> {
    use ActionKind as A;
    use TaskState as S;
    match (current, action) {
        (S::Running | S::Waiting, A::Complete) => Ok(S::Completed),
        (S::Running | S::Waiting, A::Submit) => Ok(S::Submitted),
        (S::Ready | S::Running | S::Waiting, A::Skip) => Ok(S::Skipped),
        // `Cancel` is the legacy rewind, guard included: the legacy handler
        // walks from the act up to its step and refuses unless that step's own
        // work is done (`is_biz_success` = Completed/Submitted/Backed) — a
        // cancel redoes the step and undoes the chain forward from it, which
        // only makes sense once the step ran. The kernel's action targets the
        // node the act folds into, so the same surface decides here: a
        // finished node is closed `Backed` (the legacy cancelled act's own
        // fact) and re-entered, everything else is refused.
        (S::Completed | S::Submitted, A::Cancel) => Ok(S::Backed),
        (S::Running | S::Waiting, A::Abort) => Ok(S::Aborted),
        (S::Waiting, A::Back) => Ok(S::Backed),
        (S::Running | S::Waiting, A::Fail { .. }) => Ok(S::Failed),
        (_, action) => Err(CommandError::ActionNotAllowed {
            state: current,
            action: action.clone(),
        }),
    }
}

/// Whether `action` is admissible on an execution in `state` — the guard
/// `action_target` applies, exposed so a recovering hop can tell a *live*
/// durable decision (which it must replay) from a stale one whose target moved
/// past it while the intent was in flight.
pub(crate) fn action_allowed(current: TaskState, action: &ActionKind) -> bool {
    action_target(current, action).is_ok()
}

fn action_name(action: &ActionKind) -> &'static str {
    match action {
        ActionKind::Complete => "completed",
        ActionKind::Submit => "submitted",
        ActionKind::Skip => "skipped",
        ActionKind::Cancel => "cancelled",
        ActionKind::Abort => "aborted",
        ActionKind::Back => "backed",
        ActionKind::Fail { .. } => "failed",
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CommandError {
    WrongProcess {
        expected: String,
        actual: String,
    },
    ForeignSourceOperation(OperationId),
    SourceOperationTargetsDifferentTask,
    TaskNotFound(TaskId),
    ActionNotAllowed {
        state: TaskState,
        action: ActionKind,
    },
    EmptyInputName,
    DuplicateInput(String),
    ReservedErrorInput,
    SequenceOverflow,
    InvalidPlan(String),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "command error: {self:?}")
    }
}
impl std::error::Error for CommandError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::core::commit::process::TaskSnapshot;
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
                node: "step".into(),
                generation: 2,
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
    fn command(action: ActionKind) -> ApplyActionCommand {
        ApplyActionCommand {
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
        assert_eq!(plan.operations[0].kind, OperationKind::ApplyAction);
        assert_eq!(plan.operations[0].id.generation, 2);
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
}
