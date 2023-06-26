mod action;
mod emitter;
mod message;

#[cfg(test)]
mod tests;

use crate::utils::consts;
pub use action::Action;
pub use emitter::{Emitter, Event};
pub use message::{Message, MessageKind};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub enum EventAction {
    #[default]
    Create,
    Update,
    Complete,
    Submit,
    Skip,
    Back,
    Cancel,
    Abort,
    Error,
}

impl From<&str> for EventAction {
    fn from(value: &str) -> Self {
        let ret = match value {
            consts::EVT_COMPLETE => EventAction::Complete,
            consts::EVT_BACK => EventAction::Back,
            consts::EVT_CANCEL => EventAction::Cancel,
            consts::EVT_ABORT => EventAction::Abort,
            consts::EVT_SUBMIT => EventAction::Submit,
            consts::EVT_SKIP => EventAction::Skip,
            consts::EVT_ERROR => EventAction::Error,
            consts::EVT_UPDATE => EventAction::Update,
            consts::EVT_CREATE | _ => EventAction::Create,
        };

        ret
    }
}

impl std::fmt::Display for EventAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EventAction::Create => f.write_str(consts::EVT_CREATE),
            EventAction::Update => f.write_str(consts::EVT_UPDATE),
            EventAction::Complete => f.write_str(consts::EVT_COMPLETE),
            EventAction::Back => f.write_str(consts::EVT_BACK),
            EventAction::Cancel => f.write_str(consts::EVT_CANCEL),
            EventAction::Submit => f.write_str(consts::EVT_SUBMIT),
            EventAction::Abort => f.write_str(consts::EVT_ABORT),
            EventAction::Skip => f.write_str(consts::EVT_SKIP),
            EventAction::Error => f.write_str(consts::EVT_ERROR),
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct EventData {
    pub pid: String,
    pub event: EventAction,
    // pub state: TaskState,
    // pub vars: Vars,
}

impl std::fmt::Display for EventData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&format!("pid:{}, event:{}", self.pid, self.event))
    }
}
