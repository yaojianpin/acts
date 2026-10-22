use super::*;

pub(super) fn action_target(
    current: TaskState,
    action: &ActionKind,
) -> Result<TaskState, CommandError> {
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
        // Remove — a running or waiting act is dropped; the legacy walk decided
        // the same states (`Removed` on a live task, refused on a settled one).
        (S::Running | S::Waiting, A::Remove) => Ok(S::Removed),
        (_, action) => Err(CommandError::ActionNotAllowed {
            state: current,
            action: action.clone(),
        }),
    }
}

/// Whether `action` is admissible on an execution in `state` — the guard
/// `action_target` applies, exposed so a recovering operation can tell a *live*
/// durable decision (which it must replay) from a stale one whose target moved
/// past it while the intent was in flight. `Push` decides no state: it is
/// admissible on exactly the states a decision may reach.
pub(crate) fn action_allowed(current: TaskState, action: &ActionKind) -> bool {
    if matches!(action, ActionKind::Push) {
        return matches!(current, TaskState::Running | TaskState::Waiting);
    }
    action_target(current, action).is_ok()
}
