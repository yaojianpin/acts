use super::*;

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
