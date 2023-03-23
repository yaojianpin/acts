use crate::{TaskState, Vars};

mod hub;
mod message;

pub use hub::{Event, EventHub};
pub use message::{ActionOptions, Message, UserMessage};

#[derive(Debug, Clone, PartialEq)]
pub enum EventAction {
    Create,
    Next,
    Submit,
    Skip,
    Back,
    Cancel,
    Abort,
    Error,
    Custom(String),
}

impl EventAction {
    pub fn parse(name: &str) -> EventAction {
        let ret = match name {
            "create" => EventAction::Create,
            "next" => EventAction::Next,
            "back" => EventAction::Back,
            "cancel" => EventAction::Cancel,
            "error" => EventAction::Error,
            "abort" => EventAction::Abort,
            "submit" => EventAction::Submit,
            "skip" => EventAction::Skip,
            action @ _ => EventAction::Custom(action.to_string()),
        };

        ret
    }
}

impl std::fmt::Display for EventAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EventAction::Create => f.write_str("create"),
            EventAction::Next => f.write_str("complete"),
            EventAction::Back => f.write_str("back"),
            EventAction::Cancel => f.write_str("cancel"),
            EventAction::Error => f.write_str("error"),
            EventAction::Submit => f.write_str("submit"),
            EventAction::Abort => f.write_str("abort"),
            EventAction::Skip => f.write_str("skip"),
            EventAction::Custom(name) => f.write_str(name),
        }
    }
}

#[derive(Debug, Clone)]
pub struct EventData {
    pub pid: String,
    pub action: EventAction,
    pub state: TaskState,
    pub vars: Vars,
}

impl std::fmt::Display for EventData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&format!(
            "pid:{}, action:{}, state:{}, vars:{:?}",
            self.pid, self.action, self.state, self.vars
        ))
    }
}
