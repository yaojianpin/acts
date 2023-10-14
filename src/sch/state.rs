use core::{clone::Clone, convert::From, fmt};
use serde::{Deserialize, Serialize};

use crate::utils;

#[derive(Debug, Deserialize, Serialize, Default, Clone, PartialEq)]
pub enum TaskState {
    #[default]
    None,

    Pending,
    Running,

    Success,
    Fail(String),
    Abort,
    Skip,
}

impl TaskState {
    pub fn is_none(&self) -> bool {
        *self == TaskState::None
    }
    pub fn is_completed(&self) -> bool {
        match self {
            TaskState::Success | TaskState::Fail(..) | TaskState::Skip | TaskState::Abort => true,
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
        self.is_skip() || self.is_running()
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
