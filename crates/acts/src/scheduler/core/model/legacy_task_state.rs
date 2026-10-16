use ::core::{clone::Clone, fmt};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Default, Clone, PartialEq)]
pub enum LegacyTaskState {
    /// initialized state
    #[default]
    None,

    // task is ready to run
    Ready,

    /// task is pending and waiting for the other task to wake up
    Pending,

    /// task is in running
    Running,

    /// task is interrupted and waiting for the external action to resume
    Interrupt,

    /// task is completed
    Completed,

    /// task is submitted by submit action
    Submitted,

    /// task is backed by back action
    Backed,

    /// task is cancelled by cancel action
    Cancelled,

    /// task is failed with some reasons
    Error,

    /// task is aborted by abort action
    Aborted,

    /// task is skippted by exteral action or internal conditions
    Skipped,

    /// task is removed
    Removed,
}

impl LegacyTaskState {
    pub fn is_none(&self) -> bool {
        *self == LegacyTaskState::None
    }

    pub fn is_created(&self) -> bool {
        matches!(
            self,
            LegacyTaskState::Ready | LegacyTaskState::Interrupt | LegacyTaskState::Pending
        )
    }

    /// task is completed
    /// Note: This is different from `is_biz_success`, which only includes business success states.
    pub fn is_completed(&self) -> bool {
        matches!(
            self,
            LegacyTaskState::Completed
                | LegacyTaskState::Cancelled
                | LegacyTaskState::Submitted
                | LegacyTaskState::Backed
                | LegacyTaskState::Error
                | LegacyTaskState::Skipped
                | LegacyTaskState::Aborted
                | LegacyTaskState::Removed
        )
    }

    pub fn is_abort(&self) -> bool {
        matches!(self, LegacyTaskState::Aborted)
    }

    pub fn is_error(&self) -> bool {
        matches!(self, LegacyTaskState::Error)
    }

    pub fn is_removed(&self) -> bool {
        *self == LegacyTaskState::Removed
    }

    pub fn is_ready(&self) -> bool {
        *self == LegacyTaskState::Ready
    }

    pub fn is_running(&self) -> bool {
        *self == LegacyTaskState::Running
    }

    pub fn is_pending(&self) -> bool {
        *self == LegacyTaskState::Pending
    }

    /// task is completed successfully for business purposes, including completed, submitted, backed, removed, cancelled
    ///
    /// Note: This is different from `is_completed`, which includes error, skipped, aborted, etc.
    /// the `sucess` here means the task is completed in business action successfully, and the task can be considered as done.
    /// business action means cancel, back, submit, remove, complete, etc. are all business actions.
    pub fn is_biz_success(&self) -> bool {
        *self == LegacyTaskState::Completed
            || *self == LegacyTaskState::Submitted
            || *self == LegacyTaskState::Backed
            || *self == LegacyTaskState::Removed
            || *self == LegacyTaskState::Cancelled
    }

    pub fn is_skip(&self) -> bool {
        *self == LegacyTaskState::Skipped
    }

    pub fn is_interrupted(&self) -> bool {
        *self == LegacyTaskState::Interrupt
    }
}

impl fmt::Display for LegacyTaskState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::result::Result<(), std::fmt::Error> {
        let s: String = self.into();
        f.write_str(&s)
    }
}

impl From<LegacyTaskState> for String {
    fn from(state: LegacyTaskState) -> Self {
        state_to_str(state)
    }
}

impl From<&str> for LegacyTaskState {
    fn from(str: &str) -> Self {
        str_to_state(str)
    }
}

impl From<String> for LegacyTaskState {
    fn from(str: String) -> Self {
        str_to_state(&str)
    }
}

impl From<&LegacyTaskState> for String {
    fn from(state: &LegacyTaskState) -> Self {
        state_to_str(state.clone())
    }
}

fn state_to_str(state: LegacyTaskState) -> String {
    match state {
        LegacyTaskState::Ready => "ready".to_string(),
        LegacyTaskState::Pending => "pending".to_string(),
        LegacyTaskState::Running => "running".to_string(),
        LegacyTaskState::Interrupt => "interrupted".to_string(),
        LegacyTaskState::Completed => "completed".to_string(),
        LegacyTaskState::Submitted => "submitted".to_string(),
        LegacyTaskState::Backed => "backed".to_string(),
        LegacyTaskState::Cancelled => "cancelled".to_string(),
        LegacyTaskState::Error => "error".to_string(),
        LegacyTaskState::Skipped => "skipped".to_string(),
        LegacyTaskState::Aborted => "aborted".to_string(),
        LegacyTaskState::Removed => "removed".to_string(),
        LegacyTaskState::None => "none".to_string(),
    }
}

fn str_to_state(str: &str) -> LegacyTaskState {
    match str {
        "none" => LegacyTaskState::None,
        "ready" => LegacyTaskState::Ready,
        "pending" => LegacyTaskState::Pending,
        "running" => LegacyTaskState::Running,
        "completed" => LegacyTaskState::Completed,
        "cancelled" => LegacyTaskState::Cancelled,
        "backed" => LegacyTaskState::Backed,
        "submitted" => LegacyTaskState::Submitted,
        "removed" => LegacyTaskState::Removed,
        "skipped" => LegacyTaskState::Skipped,
        "aborted" => LegacyTaskState::Aborted,
        "interrupted" => LegacyTaskState::Interrupt,
        "error" => LegacyTaskState::Error,
        _ => LegacyTaskState::None,
    }
}
