use core::{clone::Clone, fmt};
use serde::{Deserialize, Serialize};

use crate::utils;

#[derive(Debug, Deserialize, Serialize, Default, Clone, PartialEq)]
pub enum TaskState {
    /// initialized state
    #[default]
    None,

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

impl TaskState {
    pub fn is_none(&self) -> bool {
        *self == TaskState::None
    }

    pub fn is_created(&self) -> bool {
        match self {
            TaskState::Running | TaskState::Interrupt | TaskState::Pending => true,
            _ => false,
        }
    }

    pub fn is_completed(&self) -> bool {
        match self {
            TaskState::Completed
            | TaskState::Cancelled
            | TaskState::Submitted
            | TaskState::Backed
            | TaskState::Error
            | TaskState::Skipped
            | TaskState::Aborted
            | TaskState::Removed => true,
            _ => false,
        }
    }

    pub fn is_abort(&self) -> bool {
        match self {
            TaskState::Aborted => true,
            _ => false,
        }
    }

    pub fn is_error(&self) -> bool {
        match self {
            TaskState::Error => true,
            _ => false,
        }
    }

    pub fn is_removed(&self) -> bool {
        *self == TaskState::Removed
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
        utils::state_to_str(state)
    }
}

impl From<&str> for TaskState {
    fn from(str: &str) -> Self {
        utils::str_to_state(str)
    }
}

impl From<String> for TaskState {
    fn from(str: String) -> Self {
        utils::str_to_state(&str)
    }
}

impl From<&TaskState> for String {
    fn from(state: &TaskState) -> Self {
        utils::state_to_str(state.clone())
    }
}
