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
    /// Transfer — move an assigned human act's owner to another user without
    /// deciding it. The plan writes only `#assignment` (and the assignment
    /// notification); it never changes the task's state, never completes it
    /// and never creates another execution.
    Transfer,
    /// Delegate — let a business user decide the act for its owner until the
    /// delegation expires or is revoked. Like a transfer it writes assignment
    /// state only: `#delegation` plus the assignee, no state change.
    Delegate,
    /// Undelegate — revoke the act's delegation, restoring the owner as the
    /// assignee. No state change, no propagation.
    Undelegate,
    /// Remind — a non-owner urges an assigned human act. It decides nothing
    /// and moves no assignment: the plan writes the reminder record (and, on
    /// a transferred task, announces an `acts.core.msg` to the transfer
    /// target) and leaves the act exactly as it was.
    Remind,
}

impl ActionKind {
    /// Whether the action changes who may decide an act rather than deciding
    /// it. Every such action writes `#assignment`/`#delegation` and
    /// announces an `assignment` message, and is admissible on exactly the
    /// live states a decision may reach.
    pub(crate) fn moves_assignment(&self) -> bool {
        matches!(self, Self::Transfer | Self::Delegate | Self::Undelegate)
    }
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
        ActionKind::Transfer => "transferred",
        ActionKind::Delegate => "delegated",
        ActionKind::Undelegate => "undelegated",
        ActionKind::Remind => "reminded",
    }
}
