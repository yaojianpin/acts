use super::*;
use crate::scheduler::core::plan::ActPlan;

/// The join of a step that is deciding a caught error: its CATCH children
/// have settled, and [`CatchJoin`] decides the step.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CatchJoinCommand {
    pub(crate) process_id: String,
    /// The failed step whose CATCH children are settling.
    pub(crate) parent: TaskId,
    /// The CATCH child whose settlement triggered this join.
    pub(crate) source: TaskId,
}

/// What a settled CATCH child set decided.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CatchJoinOutcome {
    /// Every CATCH child skipped: no handler matched, so the error resumes
    /// (the legacy `auto_complete` re-derives it and calls `emit_error`). The
    /// step is decided `Failed` and propagates, which is what carries the
    /// error walk up.
    Resumed(StateChange, OperationIntent),
    /// At least one CATCH child did not skip: the error was caught, and the
    /// step completes (the legacy `set_state_if_running(Completed)`).
    Caught(StateChange, OperationIntent),
}

/// The catch join: decide a failed step from its settled CATCH children.
pub(crate) struct CatchJoin;

impl ActPlan for CatchJoin {
    type Reading<'a> = (&'a TaskGraph, &'a ProcessSnapshot);
    type Command<'a> = CatchJoinCommand;
    type Output = Option<CatchJoinOutcome>;
    type Error = ContainerError;

    /// The join of a step that is deciding a caught error: every child has
    /// settled and the step itself decides it from its CATCH children, exactly
    /// like the legacy `auto_complete` pass that counted the children of a
    /// failed step.
    ///
    /// The legacy rule is `is_empty_catched`: the step's `Sign::ERROR` was
    /// pinned by its failure hop, `auto_complete` waits for *every* child that
    /// exists (a `timeouts` branch the tick fired included), and then
    ///
    /// - every CATCH child skipped ⇒ no handler matched, so the error is
    ///   re-derived and `emit_error` walks it up: the step is decided
    ///   `Failed` (`CatchJoinOutcome::Resumed`);
    /// - any CATCH child that did not skip ⇒ a handler ran, so the step
    ///   completes and the error is caught (`CatchJoinOutcome::Caught`).
    ///
    /// The join is `None` — "not yet, or not mine" — while a child is still
    /// in flight, when the parent is not deciding its catches (that flow is
    /// what separates a failed step from a container parked for its children),
    /// and when a child was aborted (the abort walk owns that outcome, like the
    /// plain join). Any child's settlement may trigger it: a step that declares
    /// both `catches` and `timeouts` has timeout branches among its children,
    /// and the last one to settle is what completes the gate.
    fn plan(
        reading: (&TaskGraph, &ProcessSnapshot),
        command: &CatchJoinCommand,
    ) -> Result<Option<CatchJoinOutcome>, ContainerError> {
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
        // Only an execution whose failure hop pinned the deciding flow decides
        // a caught error, and only one of its own children settling may trigger
        // the decision.
        if !deciding_catches(snapshot, &command.parent) {
            return Ok(None);
        }
        let parent = snapshot
            .tasks
            .get(&command.parent)
            .ok_or_else(|| ContainerError::TaskNotFound(command.parent.clone()))?;
        // The failed step stays `Running` while its CATCH children run (the
        // legacy `Step::on_error`); a decision that landed meanwhile owns it.
        if parent.state != TaskState::Running {
            return Ok(None);
        }
        let children = graph.children(&command.parent);
        if children.is_empty() {
            return Ok(None);
        }
        if !children.contains(&command.source) {
            return Ok(None);
        }
        let mut resumed = true;
        let mut handlers = 0usize;
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
                // The abort walk owns this step's outcome (legacy
                // `auto_complete` refuses to decide an aborted child set).
                return Ok(None);
            }
            // Only the CATCH children decide the caught error — the legacy
            // `is_empty_catched` filters by the sign. The step's other children
            // (a `timeouts` branch the tick fired) gate the join above but do
            // not vote.
            if task_sign(snapshot, child) != Some(SIGN_CATCH) {
                continue;
            }
            handlers += 1;
            if state != TaskState::Skipped {
                resumed = false;
            }
        }
        if handlers == 0 {
            // A decider without handlers is an aggregate the failure fan-out
            // did not produce: refuse to guess a catch outcome for it.
            return Ok(None);
        }
        let sequence =
            snapshot.next_operation_sequence(&command.parent.node, command.parent.generation);
        let operation = OperationIntent {
            id: OperationId {
                process_id: command.process_id.clone(),
                node: command.parent.node.clone(),
                generation: command.parent.generation,
                sequence,
            },
            kind: OperationKind::PropagateTask,
        };
        let change = |to: TaskState| StateChange {
            task: command.parent.clone(),
            expected_revision: parent.revision,
            from: TaskState::Running,
            to,
        };
        Ok(Some(if resumed {
            CatchJoinOutcome::Resumed(change(TaskState::Failed), operation)
        } else {
            CatchJoinOutcome::Caught(change(TaskState::Completed), operation)
        }))
    }
}
