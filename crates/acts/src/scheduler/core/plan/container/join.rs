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

/// One counted child's vote: the state a join rule reads.
///
/// `Skipped` and `Removed` are *withdrawn* — a branch its own guard skipped,
/// or an act the client removed, was never in the vote, so neither the
/// completions nor the threshold count it (and `all` keeps completing over
/// one, exactly as the join always did). `Completed`/`Submitted` are
/// completions; `Failed`/`Cancelled`/`Backed` are settled non-completions
/// that can never pass; anything else is still in flight.
enum Vote {
    Completion,
    Settled,
    Withdrawn,
    InFlight,
    /// The abort walk owns the container's outcome; the join decides nothing.
    Aborted,
}

fn vote(state: TaskState) -> Vote {
    match state {
        TaskState::Completed | TaskState::Submitted => Vote::Completion,
        TaskState::Failed | TaskState::Cancelled | TaskState::Backed => Vote::Settled,
        TaskState::Skipped | TaskState::Removed => Vote::Withdrawn,
        TaskState::Aborted => Vote::Aborted,
        TaskState::Created | TaskState::Ready | TaskState::Running | TaskState::Waiting => {
            Vote::InFlight
        }
    }
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
        match command.join.as_ref() {
            None | Some(crate::JoinRule::All) => {
                Self::all_children_join(snapshot, command, parent, children)
            }
            Some(rule @ (crate::JoinRule::Any | crate::JoinRule::Partial(_))) => {
                Self::threshold_join(snapshot, command, parent, children, rule)
            }
            // The delegated rule is the shell's decision: the verdicts its
            // requests carry complete or fail the container, and the join
            // counting completions would only race them.
            Some(crate::JoinRule::Action { .. }) => Ok(None),
        }
    }
}

impl Join {
    /// The original shape: every child terminal, none aborted, then the
    /// container completes. A child failure never decides here — the failed
    /// child's own error walk decides the chain.
    fn all_children_join(
        snapshot: &ProcessSnapshot,
        command: &ContainerJoinCommand,
        parent: &crate::scheduler::core::commit::aggregate::TaskSnapshot,
        children: &[TaskId],
    ) -> Result<Option<JoinOutcome>, ContainerError> {
        for child in children {
            let state = snapshot
                .tasks
                .get(child)
                .ok_or_else(|| ContainerError::TaskNotFound(child.clone()))?
                .state;
            if !state.is_terminal() {
                return Ok(None);
            }
            if state == TaskState::Aborted {
                // The abort walk owns this container's outcome; completing it
                // here would double-decide (legacy `auto_complete` refuses).
                return Ok(None);
            }
        }
        let sequence = Self::next_sequence(snapshot, command)?;
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
            cancels: Vec::new(),
            failure: None,
        }))
    }

    /// A threshold rule: complete on `K` completions, fail once `K` can no
    /// longer be reached, wait otherwise.
    ///
    /// A settled non-completion (`Failed`, `Cancelled`, `Backed`) is a vote
    /// that can never pass — unlike `all`, the threshold rules do not fail
    /// the chain on it (the error walk is gated for them); it only shrinks
    /// what the remaining children can still reach.
    fn threshold_join(
        snapshot: &ProcessSnapshot,
        command: &ContainerJoinCommand,
        parent: &crate::scheduler::core::commit::aggregate::TaskSnapshot,
        children: &[TaskId],
        rule: &crate::JoinRule,
    ) -> Result<Option<JoinOutcome>, ContainerError> {
        let mut completions = 0usize;
        let mut counted = 0usize;
        let mut inflight: Vec<TaskId> = Vec::new();
        for child in children {
            let state = snapshot
                .tasks
                .get(child)
                .ok_or_else(|| ContainerError::TaskNotFound(child.clone()))?
                .state;
            match vote(state) {
                Vote::Aborted => return Ok(None),
                Vote::Completion => {
                    completions += 1;
                    counted += 1;
                }
                Vote::Settled => counted += 1,
                Vote::Withdrawn => {}
                Vote::InFlight => {
                    counted += 1;
                    inflight.push(child.clone());
                }
            }
        }
        let threshold = rule.threshold(counted);
        let unreachable = completions + inflight.len() < threshold;
        if completions < threshold && !unreachable {
            return Ok(None);
        }
        let sequence = Self::next_sequence(snapshot, command)?;
        let change = StateChange {
            task: command.parent.clone(),
            expected_revision: parent.revision,
            from: TaskState::Running,
            to: if unreachable {
                TaskState::Failed
            } else {
                TaskState::Completed
            },
        };
        let failure = unreachable.then(|| {
            (
                "join_failed".to_string(),
                format!(
                    "join '{rule}' unreachable: {completions} of {threshold} completions, {} of {counted} counted children settled",
                    counted - inflight.len()
                ),
            )
        });
        // Only a completion cancels the stragglers itself: a failed
        // container's propagation is a dead-source cascade, which already
        // skips its live children. A `Created` child never reaches the
        // cancel list: `SkipTask` settles admitted executions only (a
        // container's children are admitted by its own fan-out, so the case
        // cannot arise; the filter keeps the plan valid if it ever does).
        let cancels = if unreachable {
            Vec::new()
        } else {
            inflight
                .iter()
                .filter(|child| {
                    snapshot.tasks.get(*child).is_some_and(|task| {
                        matches!(
                            task.state,
                            TaskState::Ready | TaskState::Running | TaskState::Waiting
                        )
                    })
                })
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
                .collect()
        };
        Ok(Some(JoinOutcome {
            change,
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
            failure,
        }))
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
}
