use super::*;
use crate::scheduler::core::plan::ActPlan;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ContainerJoinCommand {
    pub(crate) process_id: String,
    pub(crate) parent: TaskId,
    /// The container's join rule, resolved by the host from the durable
    /// model. `None` (and [`crate::JoinRule::All`]) is the engine's
    /// original shape: every child settles, none aborted, then the container
    /// completes.
    pub(crate) join: Option<crate::JoinRule>,
}

/// What the join decided for a parked container.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct JoinOutcome {
    pub(crate) change: StateChange,
    pub(crate) operation: OperationIntent,
    /// The counted children still in flight when the rule decided early:
    /// each is handed a `SkipTask`, so the vote is withdrawn and the record
    /// closes clean instead of parking strays under a decided container.
    /// A completion cancels them itself; a failure leaves them to its own
    /// propagation, whose dead-source cascade already skips the live
    /// children (skipping them twice would refuse one).
    pub(crate) cancels: Vec<OperationIntent>,
    /// `Some((code, message))` when the rule decided the container
    /// *failed* — the threshold became unreachable. The host writes the
    /// error variables and the failure message.
    pub(crate) failure: Option<(String, String)>,
}

/// The plain container join: decide a parked container from its children —
/// every child terminal (the original rule), or a join rule's threshold of
/// completions.
pub(crate) struct Join;

impl ActPlan for Join {
    type Reading<'a> = (&'a TaskGraph, &'a ProcessSnapshot);
    type Command<'a> = ContainerJoinCommand;
    type Output = Option<JoinOutcome>;
    type Error = ContainerError;

    /// The join: when a container's children are all terminal (and none was
    /// aborted — an abort owns the container's outcome, exactly like the
    /// legacy `auto_complete`), the parked container completes and hands
    /// propagation to its own chain successor. `None` means "not yet" — or
    /// "not mine": only a `Running` container carrying the park mark joins,
    /// so a child settling under an execution that is still waiting for its
    /// own act decides nothing.
    ///
    /// With a rule, the join may decide *early*: `any` and `partial(...)`
    /// complete on the threshold of completions (the counted children still
    /// in flight are handed `SkipTask` cancels — the votes are withdrawn),
    /// and fail once the threshold can no longer be reached — every remaining
    /// child would have to complete, and there are not enough of them left.
    /// A delegated rule (`action`) decides by verdict, never by count, so the
    /// join yields it entirely to the shell.
    fn plan(
        reading: (&TaskGraph, &ProcessSnapshot),
        command: &ContainerJoinCommand,
    ) -> Result<Option<JoinOutcome>, ContainerError> {
        let (graph, snapshot) = reading;
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
            .tasks
            .get(&command.parent)
            .ok_or_else(|| ContainerError::TaskNotFound(command.parent.clone()))?;
        // A parent in its own act's flow is not a container: `Waiting` means it
        // owes the client (the legacy `Interrupt`), `Running` means its act is
        // in flight — and a child settling under either must not decide it. The
        // flow fact is what tells a container apart from those; the state alone
        // cannot, since a container is `Running` too.
        if parent.state != TaskState::Running {
            return Ok(None);
        }
        if !parked_for_children(snapshot, &command.parent) {
            return Ok(None);
        }
        let children = graph.children(&command.parent);
        if children.is_empty() {
            return Ok(None);
        }
        // A candidates request the container minted to fill its pool
        // (`<parent>#cand<k>`) is the container's own asking, not one of the
        // votes the rule counts: its *answered* request must not read as a
        // completion, or a `join: any` container completes on the answer
        // alone — before any vote ran — and an all-votes-failed round never
        // fails. A request that died (`Failed`/`Cancelled`) stays in the
        // census: the pool can never fill, which is the container's own
        // failure to propagate.
        let ask_prefix = format!("{}#cand", command.parent.node.as_str());
        let is_pool_ask = |child: &TaskId| {
            child
                .node
                .as_str()
                .strip_prefix(&ask_prefix)
                .is_some_and(|rest| {
                    !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_digit())
                })
        };
        let census: Vec<(TaskId, TaskState)> = children
            .iter()
            .filter(|child| {
                !(is_pool_ask(child)
                    && matches!(
                        snapshot.tasks.get(*child).map(|task| task.state),
                        Some(TaskState::Completed | TaskState::Submitted)
                    ))
            })
            .map(|child| {
                let state = snapshot
                    .tasks
                    .get(child)
                    .ok_or_else(|| ContainerError::TaskNotFound(child.clone()))?
                    .state;
                Ok((child.clone(), state))
            })
            .collect::<Result<_, _>>()?;
        // Every child was an answered pool ask (the answer is in, the
        // fan-out has not run yet): nothing to decide on.
        if census.is_empty() {
            return Ok(None);
        }
        let decision = super::join_rules::decide(command.join.as_ref(), &census);
        let sequence = next_sequence(snapshot, command)?;
        match decision {
            super::join_rules::Decision::Wait => Ok(None),
            super::join_rules::Decision::Complete { withdraw } => {
                let cancels = withdraw
                    .iter()
                    .map(|child| OperationIntent {
                        id: OperationId {
                            process_id: command.process_id.clone(),
                            task: child.clone(),
                            sequence: snapshot
                                .next_operation_sequence(child.node.as_str(), child.generation),
                        },
                        kind: OperationKind::SkipTask,
                        ready_at_ms: None,
                    })
                    .collect();
                Ok(Some(JoinOutcome {
                    change: StateChange {
                        task: command.parent.clone(),
                        expected_revision: parent.revision,
                        from: TaskState::Running,
                        to: TaskState::Completed,
                    },
                    operation: OperationIntent {
                        id: OperationId {
                            process_id: command.process_id.clone(),
                            task: command.parent.clone(),
                            sequence,
                        },
                        kind: OperationKind::PropagateTask,
                        ready_at_ms: None,
                    },
                    cancels,
                    failure: None,
                }))
            }
            super::join_rules::Decision::Fail { code, message } => Ok(Some(JoinOutcome {
                change: StateChange {
                    task: command.parent.clone(),
                    expected_revision: parent.revision,
                    from: TaskState::Running,
                    to: TaskState::Failed,
                },
                operation: OperationIntent {
                    id: OperationId {
                        process_id: command.process_id.clone(),
                        task: command.parent.clone(),
                        sequence,
                    },
                    kind: OperationKind::PropagateTask,
                    ready_at_ms: None,
                },
                cancels: Vec::new(),
                failure: Some((code, message)),
            })),
        }
    }
}

fn next_sequence(
    snapshot: &ProcessSnapshot,
    command: &ContainerJoinCommand,
) -> Result<u64, ContainerError> {
    snapshot
        .operations
        .keys()
        .filter(|id| id.process_id == command.process_id && id.task == command.parent)
        .map(|id| id.sequence)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or(ContainerError::SequenceOverflow)
}
