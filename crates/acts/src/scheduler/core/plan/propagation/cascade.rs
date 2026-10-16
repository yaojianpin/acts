use super::*;

/// The engine's failure vocabulary on a scope: the names the catch fan-out
/// reserves and the upward error walk carries (`ecode`/`message` are the
/// legacy task-data names, `error.code`/`error.message` the kernel's own).
fn is_error_fact(name: &str) -> bool {
    matches!(name, "ecode" | "message" | "error.code" | "error.message")
}

impl crate::scheduler::core::plan::ActPlan for Propagate {
    type Reading<'a> = (&'a TaskGraph, &'a ProcessSnapshot);
    type Command<'a> = PropagateCommand;
    type Output = TransitionPlan;
    type Error = PropagationError;

    fn plan(
        reading: Self::Reading<'_>,
        command: &Self::Command<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let (graph, snapshot) = reading;
        validate_envelope(graph, snapshot, command)?;
        let source = snapshot
            .tasks
            .get(&command.task)
            .ok_or_else(|| PropagationError::TaskNotFound(command.task.clone()))?;
        if !source.state.is_terminal() {
            return Err(PropagationError::SourceNotTerminal {
                task: command.task.clone(),
                state: source.state,
            });
        }

        let mut state_changes = Vec::new();
        let mut operations = Vec::new();
        let mut creates = Vec::new();
        let mut writes: Vec<VariableWrite> = Vec::new();
        // Whether the source's death flows on as a cascade (a dead source's
        // admitted successors join it) or the source is merely *done*: a step
        // its own guard skipped is the legacy fall-through — its flow runs on
        // into the next declared step.
        let source_alive = is_success(source.state) || is_guard_skip(snapshot, &command.task);
        // The targets this propagation flows into: the one node the model
        // names when the host decided the flow (see [`PropagateCommand::flow`]),
        // otherwise the graph's own successors.
        let targets: Vec<TaskId> = match &command.flow {
            Some(flow) => {
                let Some(latest) = newest_generation(snapshot, &flow.node) else {
                    // A `next` naming a node the run never instantiated: the
                    // model is inconsistent with the seeded graph. Refusing is
                    // the only honest answer — scheduling nothing would end
                    // the flow silently.
                    return Err(PropagationError::FlowTargetNotInstantiated(
                        flow.node.clone(),
                    ));
                };
                if flow.reopen {
                    // The flow re-enters a node that was already activated —
                    // terminal (a backward `next`, a `while` step's next turn)
                    // or even still parked for its children (the flow's own
                    // jump back into an ancestor while it waits). Only the node
                    // itself is created here — its own
                    // hops bring its forward closure alive (a container fans
                    // its children out as fresh generations), exactly like the
                    // seeded chain — with the parent and predecessors the dead
                    // generation had, so the join keeps waiting where it did.
                    let generation = latest
                        .0
                        .generation
                        .checked_add(1)
                        .ok_or(PropagationError::GenerationOverflow)?;
                    let task = TaskId::new(flow.node.clone(), generation)
                        .map_err(|_| PropagationError::EmptyFlowTarget)?;
                    // The re-entered generation inherits the dead one's edges,
                    // and it is admitted like any other execution: `Ready` with
                    // its start hop when every predecessor it inherited has
                    // settled, otherwise `Created` — the predecessor still in
                    // flight admits it through its own propagation (its
                    // successors include this execution the moment the create
                    // commits).
                    // A predecessor that *is* the parent container counts as
                    // passed: the parent parks precisely to wait for its
                    // children, and the flow re-enters inside it (the seeded
                    // chain's head is admitted the same way, as the fan-out's
                    // `Entry` child).
                    let predecessors_ready = latest.1.predecessors.iter().all(|predecessor| {
                        Some(predecessor) == latest.1.parent.as_ref()
                            || snapshot
                                .tasks
                                .get(predecessor)
                                .is_some_and(|task| task.state.is_terminal())
                    });
                    creates.push(TaskCreate {
                        task: task.clone(),
                        model_node: NodeId::parse(flow.node.clone())
                            .map_err(|_| PropagationError::EmptyFlowTarget)?,
                        parent: latest.1.parent.clone(),
                        predecessors: latest.1.predecessors.clone(),
                        initial_state: if predecessors_ready {
                            TaskState::Ready
                        } else {
                            TaskState::Created
                        },
                    });
                    if predecessors_ready {
                        operations.push(OperationIntent {
                            id: OperationId {
                                process_id: command.process_id.clone(),
                                node: flow.node.clone(),
                                generation,
                                sequence: snapshot.next_operation_sequence(&flow.node, generation),
                            },
                            kind: OperationKind::ExecuteTask,
                        });
                    }
                    // The re-entered generation's own scope carries what every
                    // other execution's does: its auto decision and its declared
                    // model variables.
                    writes.push(VariableWrite {
                        scope: task.clone(),
                        name: "auto".to_string(),
                        value: if flow.auto { "1" } else { "0" }.to_string(),
                    });
                    for (name, value) in &flow.vars {
                        writes.push(VariableWrite {
                            scope: task.clone(),
                            name: name.clone(),
                            value: value.clone(),
                        });
                    }
                    // The flow carries the source's own data into the scope the
                    // re-entered node inherits from — the legacy `Task::next`
                    // folding `parent.update_data(&outputs)` up the chain
                    // *before* the next task reads it. Without this a jump back
                    // to an ancestor would re-evaluate the ancestor's predicates
                    // against the state from before the iteration that asked for
                    // it (the iteration's own fold only reaches its immediate
                    // parent).
                    if let Some(parent) = latest.1.parent.clone() {
                        for ((scope, name), value) in &snapshot.variables {
                            if scope != &command.task
                                || crate::utils::consts::is_private_key(name)
                                || name == "auto"
                                // The failure vocabulary travels up the
                                // ancestor walk of its own hop (`plan_upward`'s
                                // carry), which writes it to every ancestor it
                                // decides: carrying it here too would fight
                                // that walk over the same scope.
                                || is_error_fact(name)
                                || writes.iter().any(|write| {
                                    write.scope == parent && write.name == *name
                                })
                            {
                                continue;
                            }
                            writes.push(VariableWrite {
                                scope: parent.clone(),
                                name: name.clone(),
                                value: value.clone(),
                            });
                        }
                    }
                    tracing::debug!(
                        node = %flow.node,
                        generation,
                        "the flow re-enters a settled node"
                    );
                    Vec::new()
                } else {
                    vec![latest.0]
                }
            }
            None => graph.successors(&command.task).to_vec(),
        };
        for successor in &targets {
            let target = snapshot
                .tasks
                .get(successor)
                .ok_or_else(|| PropagationError::TaskNotFound(successor.clone()))?;
            if target.state != TaskState::Created {
                // A committed successor is already scheduled or finished. The
                // source operation may still be completing after recovery, so
                // this is idempotent rather than an error — except when the
                // source is dead: an admitted-but-unfinished successor
                // (`Ready` queued for its start hop, `Running`/`Waiting` mid
                // execution, e.g. a branch child of a cancelled container)
                // joins the skip cascade in place instead of running on
                // behind a dead path.
                if !source_alive
                    && matches!(
                        target.state,
                        TaskState::Ready | TaskState::Running | TaskState::Waiting
                    )
                {
                    // The hop owns the state change (it re-reads the
                    // authoritative state); the plan only schedules it. An
                    // aborted source's `Running` successor is `Aborted` (the
                    // legacy ancestor walk aborts running children); every
                    // other death skips its successors.
                    let kind = if source.state == TaskState::Aborted
                        && target.state == TaskState::Running
                    {
                        OperationKind::AbortTask
                    } else {
                        OperationKind::SkipTask
                    };
                    operations.push(OperationIntent {
                        id: OperationId {
                            process_id: command.process_id.clone(),
                            node: successor.node.clone(),
                            generation: successor.generation,
                            sequence: snapshot
                                .next_operation_sequence(&successor.node, successor.generation),
                        },
                        kind,
                    });
                    tracing::debug!(
                        successor = ?successor,
                        source_state = ?source.state,
                        kind = ?kind,
                        "dead source; admitted successor joins the cascade"
                    );
                }
                continue;
            }
            let readiness = readiness(graph, snapshot, successor, command.join_policy)?;
            match readiness {
                JoinReadiness::Ready => {
                    state_changes.push(StateChange {
                        task: successor.clone(),
                        expected_revision: target.revision,
                        from: TaskState::Created,
                        to: TaskState::Ready,
                    });
                    operations.push(OperationIntent {
                        id: OperationId {
                            process_id: command.process_id.clone(),
                            node: successor.node.clone(),
                            generation: successor.generation,
                            sequence: snapshot
                                .next_operation_sequence(&successor.node, successor.generation),
                        },
                        kind: OperationKind::ExecuteTask,
                    });
                }
                JoinReadiness::Waiting => {}
                // A dead predecessor (cancelled/failed/aborted) never blocks
                // the run: the successor is admitted and immediately handed a
                // skip cascade, which flows the deadness past it one hop per
                // operation.
                JoinReadiness::SkipCascade { predecessor, state } => {
                    state_changes.push(StateChange {
                        task: successor.clone(),
                        expected_revision: target.revision,
                        from: TaskState::Created,
                        to: TaskState::Ready,
                    });
                    operations.push(OperationIntent {
                        id: OperationId {
                            process_id: command.process_id.clone(),
                            node: successor.node.clone(),
                            generation: successor.generation,
                            sequence: snapshot
                                .next_operation_sequence(&successor.node, successor.generation),
                        },
                        kind: OperationKind::SkipTask,
                    });
                    tracing::debug!(
                        successor = ?successor,
                        predecessor = ?predecessor,
                        predecessor_state = ?state,
                        "join predecessor is dead; successor enters the skip cascade"
                    );
                }
            }
        }

        let plan = TransitionPlan {
            process_id: command.process_id.clone(),
            expected: snapshot.version.clone(),
            task_creates: creates,
            state_changes,
            variable_writes: writes,
            source_operation: Some(command.source_operation.clone()),
            operations,
            outbox: vec![OutboxIntent {
                key: format!(
                    "propagate:{}:{}:{}:{}",
                    command.process_id,
                    command.task.node,
                    command.task.generation,
                    command.source_operation.id.sequence,
                ),
                topic: "task.propagated".into(),
                payload: format!("{}:{}", command.task.node, command.task.generation),
                receipt: None,
            }],
        };
        plan.validate()
            .map_err(|error| PropagationError::InvalidPlan(error.to_string()))?;
        Ok(plan)
    }
}

/// The newest generation of one model node, with its snapshot. `None` when the
/// run never instantiated it.
fn newest_generation<'a>(
    snapshot: &'a ProcessSnapshot,
    node: &str,
) -> Option<(
    TaskId,
    &'a crate::scheduler::core::commit::aggregate::TaskSnapshot,
)> {
    snapshot
        .tasks
        .iter()
        .filter(|(id, _)| id.node == node)
        .max_by_key(|(id, _)| id.generation)
        .map(|(id, task)| (id.clone(), task))
}

fn validate_envelope(
    graph: &TaskGraph,
    snapshot: &ProcessSnapshot,
    command: &PropagateCommand,
) -> Result<(), PropagationError> {
    if graph.process_id != command.process_id || snapshot.process_id != command.process_id {
        return Err(PropagationError::WrongProcess);
    }
    if graph.revision != snapshot.version.graph {
        return Err(PropagationError::GraphVersionConflict {
            graph: graph.revision,
            snapshot: snapshot.version.graph,
        });
    }
    let source = &command.source_operation.id;
    if source.process_id != command.process_id
        || source.node != command.task.node
        || source.generation != command.task.generation
    {
        return Err(PropagationError::SourceOperationTargetsDifferentTask);
    }
    if graph.node(&command.task).is_none() {
        return Err(PropagationError::TaskNotInGraph(command.task.clone()));
    }
    Ok(())
}
