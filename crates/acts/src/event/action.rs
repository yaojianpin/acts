use crate::Vars;
use crate::event::EventAction;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Action {
    pub id: String,
    pub pid: String,
    pub tid: String,
    pub event: EventAction,
    pub options: Vars,
}

impl Action {
    pub fn new(pid: &str, tid: &str, event: EventAction, options: Vars) -> Self {
        Self {
            id: String::new(),
            pid: pid.to_string(),
            tid: tid.to_string(),
            event,
            options,
        }
    }
}
