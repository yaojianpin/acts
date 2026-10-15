use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ContainerFanoutCommand {
    pub(crate) process_id: String,
    pub(crate) source: ExecutionId,
    pub(crate) source_operation: OperationCommit,
    pub(crate) children: Vec<ContainerChild>,
    /// Client inputs of the completion that fans out (an auto completion has
    /// none). They land on the container's own scope, exactly like the
    /// action-variable writes of a plain completion.
    pub(crate) inputs: Vec<(String, String)>,
    /// Where the source parks while its children run: `Running` for every
    /// container — the legacy step stays running while `check_in_children`'s
    /// children run, `auto_complete` finishes it once they settled, and the
    /// failed step dispatching its CATCH children is running too
    /// (`Step::on_error`). A source that is `Waiting` when it fans out (its
    /// act was just answered) moves to `Running`, exactly like the legacy step
    /// resuming its `next`.
    ///
    /// `Waiting` stays a valid park value only for the timeout dispatch, which
    /// admits a branch under an execution that keeps waiting on its own act.
    pub(crate) source_park: ExecutionState,
}

impl ContainerPlanner {
    /// Fan the container's children out in one atomic plan. The source parks
    /// in `command.source_park` — `Running` for every container (the legacy
    /// step stays running while its children run, and a failed step
    /// dispatching its CATCH children is running too); the join (see
    /// [`Self::plan_join`] /
    /// [`Self::plan_catch_join`]) decides it once every child is terminal. An
    /// empty container completes immediately.
    pub(crate) fn plan_fanout(
        snapshot: &ProcessSnapshot,
        command: &ContainerFanoutCommand,
    ) -> Result<TransitionPlan, ContainerError> {
        if snapshot.process_id != command.process_id {
            return Err(ContainerError::WrongProcess);
        }
        if !matches!(
            command.source_park,
            ExecutionState::Running | ExecutionState::Waiting
        ) {
            return Err(ContainerError::InvalidSourcePark(command.source_park));
        }
        let source = snapshot
            .executions
            .get(&command.source)
            .ok_or_else(|| ContainerError::ExecutionNotFound(command.source.clone()))?;
        if !matches!(
            source.state,
            ExecutionState::Running | ExecutionState::Waiting
        ) {
            return Err(ContainerError::SourceNotContainer {
                source: command.source.clone(),
                state: source.state,
            });
        }
        let operation = &command.source_operation.id;
        if operation.process_id != command.process_id
            || operation.task_id != command.source.task_id
            || operation.generation != command.source.generation
        {
            return Err(ContainerError::SourceOperationTargetsDifferentExecution);
        }
        if command.children.is_empty() {
            // A container without children is a normal completion: the join
            // of zero children fires in the same hop.
            return Self::empty_container_plan(snapshot, command, source.state, source.revision);
        }

        let mut creates = Vec::new();
        // The source parks in `command.source_park` — unless a client
        // completion found it already parked, in which case the state is
        // already the parked one.
        let mut state_changes = Vec::new();
        if source.state != command.source_park {
            state_changes.push(StateChange {
                execution: command.source.clone(),
                expected_revision: source.revision,
                from: source.state,
                to: command.source_park,
            });
        }
        let mut variable_writes: Vec<VariableWrite> = command
            .inputs
            .iter()
            .map(|(name, value)| VariableWrite {
                scope: command.source.clone(),
                name: name.clone(),
                value: value.clone(),
            })
            .collect();
        let mut operations = Vec::new();
        // Resolved execution of each already-processed child, for sibling
        // predecessor references.
        let mut resolved: Vec<ExecutionId> = Vec::with_capacity(command.children.len());
        // The children this source already holds, per model node, oldest
        // generation first. A child slot belongs to its *parent*: the same model
        // node may be live under several parents at once (a `uses`-param
        // container's per-item templates each instantiate the same act node), and
        // only the executions this fan-out admitted are its own to reuse. A
        // command that names one node N times (the param fan-out) maps its i-th
        // occurrence to the i-th child it already holds, and allocates the rest.
        let mut held: std::collections::HashMap<String, Vec<ExecutionId>> =
            std::collections::HashMap::with_capacity(command.children.len());
        let mut occurrence: std::collections::HashMap<String, usize> =
            std::collections::HashMap::with_capacity(command.children.len());
        // The generations this plan itself allocated, per task id: the snapshot
        // does not hold this plan's creations yet, so a repeat takes the
        // generation after the plan's own last allocation, never the one the
        // snapshot's `next_generation` reports.
        let mut allocated: std::collections::HashMap<String, u64> =
            std::collections::HashMap::with_capacity(command.children.len());
        for (index, child) in command.children.iter().enumerate() {
            if child.task_id.trim().is_empty() {
                return Err(ContainerError::EmptyChildTaskId);
            }
            let nth = {
                let count = occurrence.entry(child.task_id.clone()).or_insert(0);
                let nth = *count;
                *count += 1;
                nth
            };
            let siblings = held
                .entry(child.task_id.clone())
                .or_insert_with(|| existing_children(snapshot, &command.source, &child.task_id));
            let existing = siblings.get(nth).cloned();
            let observation =
                SlotObservation::vacant(false, false).with_existing(match &existing {
                    Some(execution) => generation_state(snapshot, execution),
                    None => ExistingGeneration::Absent,
                });
            let base = next_generation(snapshot, &child.task_id)
                .ok_or(ContainerError::GenerationOverflow)?;
            let generation = match allocated.get(&child.task_id) {
                Some(previous) => base.max(
                    previous
                        .checked_add(1)
                        .ok_or(ContainerError::GenerationOverflow)?,
                ),
                None => base,
            };
            let execution = match crate::scheduler::core::plan::execution_admission::GenerationAdmissionPolicy::decide(
                crate::scheduler::core::plan::execution_admission::ExecutionSlotKind::Once,
                crate::scheduler::core::plan::execution_admission::AdmissionIntent::Forward,
                observation,
            ) {
                crate::scheduler::core::plan::execution_admission::AdmissionDecision::Ignore => {
                    resolved.push(
                        existing.ok_or(ContainerError::ExecutionNotFound(command.source.clone()))?,
                    );
                    continue;
                }
                crate::scheduler::core::plan::execution_admission::AdmissionDecision::AllocateGeneration => {
                    let execution = ExecutionId::new(child.task_id.clone(), generation)
                        .map_err(|_| ContainerError::EmptyChildTaskId)?;
                    allocated.insert(child.task_id.clone(), generation);
                    creates.push(ExecutionCreate {
                        execution: execution.clone(),
                        model_node: child.model_node.clone(),
                        parent: Some(command.source.clone()),
                        predecessors: resolve_predecessors(
                            &child.predecessors,
                            &command.source,
                            &resolved,
                            index,
                        )?,
                        initial_state: match child.plan {
                            ContainerChildPlan::Queued => ExecutionState::Created,
                            ContainerChildPlan::Entry | ContainerChildPlan::Skip => {
                                ExecutionState::Ready
                            }
                        },
                    });
                    if let Some(intent) = child_operation(command, &execution, child.plan)? {
                        operations.push(intent);
                    }
                    child_variables(&mut variable_writes, &execution, child);
                    execution
                }
                crate::scheduler::core::plan::execution_admission::AdmissionDecision::ReadmitExisting => {
                    let execution =
                        existing.ok_or(ContainerError::ExecutionNotFound(command.source.clone()))?;
                    // A queued chain successor is admitted by propagation, never
                    // by the fan-out: an execution the seed (the root's own
                    // chain) or a rewind already created stays `Created` until
                    // its predecessor settles. Readmitting it would hand it
                    // `Ready` with no operation to run — and take the admission
                    // the propagation is about to perform.
                    if child.plan == ContainerChildPlan::Queued {
                        resolved.push(execution);
                        continue;
                    }
                    // The latest generation exists but never ran (a Cancel
                    // rewind recreated it): admit it in place instead of
                    // allocating a second live generation.
                    if let Some(change) = readmit_change(snapshot, &execution)? {
                        state_changes.push(change);
                    }
                    if let Some(intent) = child_operation(command, &execution, child.plan)? {
                        operations.push(intent);
                    }
                    child_variables(&mut variable_writes, &execution, child);
                    execution
                }
            };
            resolved.push(execution);
        }

        let plan = TransitionPlan {
            process_id: command.process_id.clone(),
            expected: snapshot.version.clone(),
            execution_creates: creates,
            state_changes,
            variable_writes,
            source_operation: Some(command.source_operation.clone()),
            operations,
            outbox: vec![fanout_outbox(command, "selected")],
        };
        plan.validate()
            .map_err(|error| ContainerError::InvalidPlan(error.to_string()))?;
        Ok(plan)
    }

    fn empty_container_plan(
        snapshot: &ProcessSnapshot,
        command: &ContainerFanoutCommand,
        state: ExecutionState,
        revision: u64,
    ) -> Result<TransitionPlan, ContainerError> {
        let sequence = command
            .source_operation
            .id
            .sequence
            .checked_add(1)
            .ok_or(ContainerError::SequenceOverflow)?;
        let mut operations = Vec::new();
        let propagate_id = OperationId {
            process_id: command.process_id.clone(),
            task_id: command.source.task_id.clone(),
            generation: command.source.generation,
            sequence,
        };
        if !snapshot.operations.contains_key(&propagate_id) {
            operations.push(OperationIntent {
                id: propagate_id,
                kind: OperationKind::PropagateTask,
            });
        }
        let plan = TransitionPlan {
            process_id: command.process_id.clone(),
            expected: snapshot.version.clone(),
            execution_creates: Vec::new(),
            state_changes: vec![StateChange {
                execution: command.source.clone(),
                expected_revision: revision,
                from: state,
                to: ExecutionState::Completed,
            }],
            variable_writes: Vec::new(),
            source_operation: Some(command.source_operation.clone()),
            operations,
            outbox: vec![fanout_outbox(command, "empty")],
        };
        plan.validate()
            .map_err(|error| ContainerError::InvalidPlan(error.to_string()))?;
        Ok(plan)
    }
}

/// The executions one task already has under one parent, oldest generation
/// first: the children a re-run of this fan-out reuses (the seed's chain, a
/// Cancel rewind's recreation) instead of duplicating.
fn existing_children(
    snapshot: &ProcessSnapshot,
    parent: &ExecutionId,
    task_id: &str,
) -> Vec<ExecutionId> {
    let mut children: Vec<ExecutionId> = snapshot
        .executions
        .iter()
        .filter(|(id, execution)| {
            id.task_id == task_id && execution.parent.as_ref() == Some(parent)
        })
        .map(|(id, _)| id.clone())
        .collect();
    children.sort_by_key(|id| id.generation);
    children
}

/// The admission observation for one already-existing execution: `Created`
/// never ran (`Unstarted`), so a rewound-and-recreated child is readmitted
/// rather than duplicated.
fn generation_state(snapshot: &ProcessSnapshot, execution: &ExecutionId) -> ExistingGeneration {
    match snapshot.executions.get(execution).map(|held| held.state) {
        Some(ExecutionState::Created) => ExistingGeneration::Unstarted,
        Some(ExecutionState::Ready | ExecutionState::Running | ExecutionState::Waiting) => {
            ExistingGeneration::Active
        }
        _ => ExistingGeneration::Completed,
    }
}

fn next_generation(snapshot: &ProcessSnapshot, task_id: &str) -> Option<u64> {
    snapshot
        .executions
        .keys()
        .filter(|id| id.task_id == task_id)
        .map(|id| id.generation)
        .max()
        .map_or(Some(0), |value| value.checked_add(1))
}

/// Turn symbolic predecessor references into execution ids: the container
/// itself, or a sibling processed earlier in the same command (its resolved
/// id, created or readmitted).
fn resolve_predecessors(
    references: &[ContainerPredecessor],
    source: &ExecutionId,
    resolved: &[ExecutionId],
    current: usize,
) -> Result<Vec<ExecutionId>, ContainerError> {
    references
        .iter()
        .map(|reference| match *reference {
            ContainerPredecessor::Source => Ok(source.clone()),
            ContainerPredecessor::Sibling(index) => {
                if index >= current {
                    return Err(ContainerError::ForwardSibling(index));
                }
                resolved
                    .get(index)
                    .cloned()
                    .ok_or(ContainerError::ForwardSibling(index))
            }
        })
        .collect()
}

/// The admission operation of one child, when its plan needs one.
fn child_operation(
    command: &ContainerFanoutCommand,
    execution: &ExecutionId,
    plan: ContainerChildPlan,
) -> Result<Option<OperationIntent>, ContainerError> {
    let kind = match plan {
        ContainerChildPlan::Queued => return Ok(None),
        ContainerChildPlan::Entry => OperationKind::ExecuteTask,
        ContainerChildPlan::Skip => OperationKind::SkipTask,
    };
    Ok(Some(OperationIntent {
        id: OperationId {
            process_id: command.process_id.clone(),
            task_id: execution.task_id.clone(),
            generation: execution.generation,
            sequence: 0,
        },
        kind,
    }))
}

fn child_variables(
    writes: &mut Vec<VariableWrite>,
    execution: &ExecutionId,
    child: &ContainerChild,
) {
    writes.push(VariableWrite {
        scope: execution.clone(),
        name: "auto".to_string(),
        value: if child.auto { "1" } else { "0" }.to_string(),
    });
    for (name, value) in &child.vars {
        writes.push(VariableWrite {
            scope: execution.clone(),
            name: name.clone(),
            value: value.clone(),
        });
    }
}

/// A readmitted child is `Created`; the admission moves it to `Ready`. An
/// unexpected state is refused rather than forced.
fn readmit_change(
    snapshot: &ProcessSnapshot,
    execution: &ExecutionId,
) -> Result<Option<StateChange>, ContainerError> {
    let existing = snapshot
        .executions
        .get(execution)
        .ok_or_else(|| ContainerError::ExecutionNotFound(execution.clone()))?;
    if existing.state != ExecutionState::Created {
        return Err(ContainerError::ReadmitNotCreated {
            execution: execution.clone(),
            state: existing.state,
        });
    }
    Ok(Some(StateChange {
        execution: execution.clone(),
        expected_revision: existing.revision,
        from: ExecutionState::Created,
        to: ExecutionState::Ready,
    }))
}

fn fanout_outbox(command: &ContainerFanoutCommand, outcome: &str) -> OutboxIntent {
    OutboxIntent {
        key: format!(
            "fanout:{}:{}:{}:{}",
            command.process_id,
            command.source.task_id,
            command.source.generation,
            command.source_operation.id.sequence,
        ),
        topic: format!("fanout.{outcome}"),
        payload: format!("{}:{}", command.source.task_id, command.source.generation),
        receipt: None,
    }
}
