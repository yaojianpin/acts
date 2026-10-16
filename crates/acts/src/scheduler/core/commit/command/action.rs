#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ActionKind {
    Complete,
    Submit,
    Skip,
    Cancel,
    Abort,
    Back,
    Fail { code: String, message: String },
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
    }
}
