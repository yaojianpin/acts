use crate::{utils, Vars};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Action {
    pub pid: String,
    pub tid: String,
    pub event: String,
    pub options: Vars,
}

impl Action {
    pub fn new(pid: &str, tid: &str, event: &str, options: &Vars) -> Self {
        Self {
            pid: pid.to_string(),
            tid: tid.to_string(),
            event: event.to_string(),
            options: options.clone(),
        }
    }

    pub fn id(&self) -> String {
        utils::Id::new(&self.pid, &self.tid).id()
    }
}
