use crate::{TaskState, Vars};

#[derive(Debug, Clone, PartialEq)]
pub enum EventAction {
    Create,
    Next,
    Back,
    Cancel,
    Error,
}

impl std::fmt::Display for EventAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EventAction::Create => f.write_str("create"),
            EventAction::Next => f.write_str("next"),
            EventAction::Back => f.write_str("back"),
            EventAction::Cancel => f.write_str("cancel"),
            EventAction::Error => f.write_str("error"),
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

mod hub;
mod message;

pub use hub::{Event, EventHub};
pub use message::{Message, UserData};
