use core::{clone::Clone, convert::From, fmt};
use serde::{Deserialize, Serialize};

use crate::{utils, Error};

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

    /// task is completed with success
    Success,

    /// task is failed with some reasons
    Fail(String),

    /// task is aborted by external action
    Abort,

    /// task is skippted by exteral action or internal conditions
    Skip,

    /// task is removed
    Removed,
}

impl TaskState {
    pub fn is_none(&self) -> bool {
        *self == TaskState::None
    }
    pub fn is_completed(&self) -> bool {
        match self {
            TaskState::Success
            | TaskState::Fail(..)
            | TaskState::Skip
            | TaskState::Abort
            | TaskState::Removed => true,
            _ => false,
        }
    }

    pub fn is_abort(&self) -> bool {
        match self {
            TaskState::Abort => true,
            _ => false,
        }
    }

    pub fn is_error(&self) -> bool {
        match self {
            TaskState::Fail(..) => true,
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
        *self == TaskState::Success
    }

    pub fn is_skip(&self) -> bool {
        *self == TaskState::Skip
    }

    pub fn is_next(&self) -> bool {
        self.is_skip() || self.is_running() || self.is_removed()
    }

    pub fn is_interrupted(&self) -> bool {
        *self == TaskState::Interrupt
    }

    pub fn as_err(&self) -> Option<Error> {
        if let TaskState::Fail(err) = self {
            return Some(Error::parse(err));
        }

        None
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
