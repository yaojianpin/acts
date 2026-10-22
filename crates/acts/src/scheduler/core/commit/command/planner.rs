use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ActionCommand {
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
    pub(crate) fn validate_command(command: &ActionCommand) -> Result<(), CommandError> {
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
        command: &ActionCommand,
    ) -> Result<TransitionPlan, CommandError> {
        Self::validate_command(command)?;
        validate_envelope(snapshot, command)?;
        let current = snapshot
            .tasks
            .get(&command.task)
            .ok_or_else(|| CommandError::TaskNotFound(command.task.clone()))?;
        // `Push` decides no state: its preflight is the admissibility guard
        // alone, and the plan that creates the pushed act is the shell's own
        // (`KernelHost::push_plan`) — the client inputs describe the new act,
        // they are not the target's scope writes.
        if matches!(
            command.action,
            crate::scheduler::core::commit::command::ActionKind::Push
        ) {
            if !crate::scheduler::core::commit::command::action_allowed(
                current.state,
                &command.action,
            ) {
                return Err(CommandError::ActionNotAllowed {
                    state: current.state,
                    action: command.action.clone(),
                });
            }
            return Self::bare_plan(snapshot, command);
        }
        let target = action_target(current.state, &command.action)?;
        let writes = variable_writes(command)?;
        let next_sequence = command
            .source_operation
            .id
            .sequence
            .checked_add(1)
            .ok_or(CommandError::SequenceOverflow)?;
        // A decision hands its propagation to the planner's pure hand-off,
        // which the committing operation folds into its own transition. `Back`
        // has nothing to propagate: it closes the source generation (`Backed`)
        // and its re-entry is the `ReenterTask` the deciding call plans two
        // sequences on (the slot the propagation hand-off would have taken is
        // kept, so the rewind's outbox key does not move).
        let operations = if matches!(command.action, ActionKind::Back) {
            Vec::new()
        } else {
            vec![OperationIntent {
                id: OperationId {
                    process_id: command.process_id.clone(),
                    task: command.task.clone(),
                    sequence: next_sequence,
                },
                kind: OperationKind::PropagateTask,
                ready_at_ms: None,
            }]
        };
        let mut plan = Self::bare_plan(snapshot, command)?;
        plan.state_changes = vec![StateChange {
            task: command.task.clone(),
            expected_revision: current.revision,
            from: current.state,
            to: target,
        }];
        plan.variable_writes = writes;
        plan.operations = operations;
        plan.outbox = vec![OutboxIntent {
            key: format!(
                "action:{}:{}:{}:{}",
                command.process_id,
                command.task.node.as_str(),
                command.task.generation,
                command.source_operation.id.sequence
            ),
            topic: format!("task.{}", action_name(&command.action)),
            payload: format!("{}:{}", command.task.node.as_str(), command.task.generation),
        }];
        plan.validate()
            .map_err(|error| CommandError::InvalidPlan(error.to_string()))?;
        Ok(plan)
    }

    /// The envelope every decision plan shares: the version guard and the
    /// source operation's own commit. A `Push` preflight is exactly this —
    /// the state table decided nothing and the client inputs belong to the
    /// pushed act, not to the target's scope.
    fn bare_plan(
        snapshot: &ProcessSnapshot,
        command: &ActionCommand,
    ) -> Result<TransitionPlan, CommandError> {
        let plan = TransitionPlan {
            process_id: command.process_id.clone(),
            expected: snapshot.version.clone(),
            task_creates: Vec::new(),
            state_changes: Vec::new(),
            variable_writes: Vec::new(),
            source_operation: Some(command.source_operation.clone()),
            operations: Vec::new(),
            outbox: Vec::new(),
        };
        plan.validate()
            .map_err(|error| CommandError::InvalidPlan(error.to_string()))?;
        Ok(plan)
    }
}

fn validate_envelope(
    snapshot: &ProcessSnapshot,
    command: &ActionCommand,
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
    if source.task != command.task {
        return Err(CommandError::SourceOperationTargetsDifferentTask);
    }
    Ok(())
}

fn variable_writes(command: &ActionCommand) -> Result<Vec<VariableWrite>, CommandError> {
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
