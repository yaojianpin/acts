use crate::Vars;

use super::EventAction;

#[derive(Clone, Debug)]
pub struct Action {
    pub pid: String,
    pub aid: String,
    pub event: String,
    pub options: Vars,
}

impl Action {
    pub fn new(pid: &str, aid: &str, event: &str, options: &Vars) -> Self {
        Self {
            pid: pid.to_string(),
            aid: aid.to_string(),
            event: event.to_string(),
            options: options.clone(),
        }
    }

    pub fn event(&self) -> EventAction {
        self.event.as_str().into()
    }
}
