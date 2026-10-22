#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ActionKind {
    Complete,
    Submit,
    Skip,
    Cancel,
    Abort,
    Back,
    Fail {
        code: String,
        message: String,
    },
    /// Push — add an act (an approver) under the execution: the client names the act
    /// (`uses`, `params`, …) and the kernel creates a child execution for it.
    /// The target itself is not decided; it waits for the pushed act to
    /// settle before its own completion joins.
    Push,
    /// Remove — drop an act: the execution is decided `Removed` and its flow
    /// continues, exactly like the legacy `task.removed` walk.
    Remove,
}

pub(super) fn action_name(action: &ActionKind) -> &'static str {
    match action {
        ActionKind::Complete => "completed",
        ActionKind::Submit => "submitted",
        ActionKind::Skip => "skipped",
        ActionKind::Cancel => "cancelled",
        ActionKind::Abort => "aborted",
        ActionKind::Back => "backed",
        ActionKind::Fail { .. } => "failed",
        ActionKind::Push => "pushed",
        ActionKind::Remove => "removed",
    }
}
