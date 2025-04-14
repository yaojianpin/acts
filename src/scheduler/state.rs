use core::{clone::Clone, fmt};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Default, Clone, PartialEq)]
pub enum TaskState {
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

    /// task is skipped by external action or internal conditions
    Skipped,

    /// task is removed
    Removed,
}

impl TaskState {
    pub fn is_none(&self) -> bool {
        *self == TaskState::None
    }

    pub fn is_created(&self) -> bool {
        matches!(
            self,
            TaskState::Ready | TaskState::Interrupt | TaskState::Pending
        )
    }

    pub fn is_completed(&self) -> bool {
        matches!(
            self,
            TaskState::Completed
                | TaskState::Cancelled
                | TaskState::Submitted
                | TaskState::Backed
                | TaskState::Error
                | TaskState::Skipped
                | TaskState::Aborted
                | TaskState::Removed
        )
    }

    pub fn is_abort(&self) -> bool {
        matches!(self, TaskState::Aborted)
    }

    pub fn is_error(&self) -> bool {
        matches!(self, TaskState::Error)
    }

    pub fn is_removed(&self) -> bool {
        *self == TaskState::Removed
    }

    pub fn is_ready(&self) -> bool {
        *self == TaskState::Ready
    }

    pub fn is_running(&self) -> bool {
        *self == TaskState::Running
    }

    pub fn is_pending(&self) -> bool {
        *self == TaskState::Pending
    }

    pub fn is_success(&self) -> bool {
        *self == TaskState::Completed
    }

    pub fn is_skip(&self) -> bool {
        *self == TaskState::Skipped
    }

    pub fn is_next(&self) -> bool {
        self.is_skip() || self.is_running() || self.is_removed()
    }

    pub fn is_interrupted(&self) -> bool {
        *self == TaskState::Interrupt
    }
}

impl fmt::Display for TaskState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::result::Result<(), std::fmt::Error> {
        let s: String = self.into();
        f.write_str(&s)
    }
}

impl From<TaskState> for String {
    fn from(state: TaskState) -> Self {
        state_to_str(state)
    }
}

impl From<&str> for TaskState {
    fn from(str: &str) -> Self {
        str_to_state(str)
    }
}

impl From<String> for TaskState {
    fn from(str: String) -> Self {
        str_to_state(&str)
    }
}

impl From<&TaskState> for String {
    fn from(state: &TaskState) -> Self {
        state_to_str(state.clone())
    }
}

fn state_to_str(state: TaskState) -> String {
    match state {
        TaskState::Ready => "ready".to_string(),
        TaskState::Pending => "pending".to_string(),
        TaskState::Running => "running".to_string(),
        TaskState::Interrupt => "interrupted".to_string(),
        TaskState::Completed => "completed".to_string(),
        TaskState::Submitted => "submitted".to_string(),
        TaskState::Backed => "backed".to_string(),
        TaskState::Cancelled => "cancelled".to_string(),
        TaskState::Error => "error".to_string(),
        TaskState::Skipped => "skipped".to_string(),
        TaskState::Aborted => "aborted".to_string(),
        TaskState::Removed => "removed".to_string(),
        TaskState::None => "none".to_string(),
    }
}

fn str_to_state(str: &str) -> TaskState {
    match str {
        "none" => TaskState::None,
        "ready" => TaskState::Ready,
        "pending" => TaskState::Pending,
        "running" => TaskState::Running,
        "completed" => TaskState::Completed,
        "cancelled" => TaskState::Cancelled,
        "backed" => TaskState::Backed,
        "submitted" => TaskState::Submitted,
        "removed" => TaskState::Removed,
        "skipped" => TaskState::Skipped,
        "aborted" => TaskState::Aborted,
        "interrupted" => TaskState::Interrupt,
        "error" => TaskState::Error,
        _ => TaskState::None,
    }
}
