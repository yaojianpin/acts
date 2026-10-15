use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ContainerJoinCommand {
    pub(crate) process_id: String,
    pub(crate) parent: ExecutionId,
}

impl ContainerPlanner {
    /// The join: when a container's children are all terminal (and none was
    /// aborted — an abort owns the container's outcome, exactly like the
    /// legacy `auto_complete`), the parked container completes and hands
    /// propagation to its own chain successor. `None` means "not yet" — or
    /// "not mine": only a `Running` container carrying the park mark joins,
    /// so a child settling under an execution that is still waiting for its
    /// own act decides nothing.
    pub(crate) fn plan_join(
        graph: &ExecutionGraph,
        snapshot: &ProcessSnapshot,
        command: &ContainerJoinCommand,
    ) -> Result<Option<(StateChange, OperationIntent)>, ContainerError> {
        if snapshot.process_id != command.process_id || graph.process_id != command.process_id {
            return Err(ContainerError::WrongProcess);
        }
        if graph.revision != snapshot.version.graph {
            return Err(ContainerError::GraphVersionConflict {
                graph: graph.revision,
                snapshot: snapshot.version.graph,
            });
        }
        let parent = snapshot
            .executions
            .get(&command.parent)
            .ok_or_else(|| ContainerError::ExecutionNotFound(command.parent.clone()))?;
        // A parent in its own act's flow is not a container: `Waiting` means it
        // owes the client (the legacy `Interrupt`), `Running` means its act is
        // in flight — and a child settling under either must not decide it. The
        // flow fact is what tells a container apart from those; the state alone
        // cannot, since a container is `Running` too.
        if parent.state != ExecutionState::Running {
            return Ok(None);
        }
        if !parked_for_children(snapshot, &command.parent) {
            return Ok(None);
        }
        let children = graph.children(&command.parent);
        if children.is_empty() {
            return Ok(None);
        }
        for child in children {
            let state = snapshot
                .executions
                .get(child)
                .ok_or_else(|| ContainerError::ExecutionNotFound(child.clone()))?
                .state;
            if !state.is_terminal() {
                return Ok(None);
            }
            if state == ExecutionState::Aborted {
                // The abort walk owns this container's outcome; completing it
                // here would double-decide (legacy `auto_complete` refuses).
                return Ok(None);
            }
        }
        let sequence = snapshot
            .operations
            .keys()
            .filter(|id| {
                id.process_id == command.process_id
                    && id.task_id == command.parent.task_id
                    && id.generation == command.parent.generation
            })
            .map(|id| id.sequence)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(ContainerError::SequenceOverflow)?;
        Ok(Some((
            StateChange {
                execution: command.parent.clone(),
                expected_revision: parent.revision,
                from: ExecutionState::Running,
                to: ExecutionState::Completed,
            },
            OperationIntent {
                id: OperationId {
                    process_id: command.process_id.clone(),
                    task_id: command.parent.task_id.clone(),
                    generation: command.parent.generation,
                    sequence,
                },
                kind: OperationKind::PropagateTask,
            },
        )))
    }
}
