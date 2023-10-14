use crate::Vars;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Action {
    pub proc_id: String,
    pub task_id: String,
    pub event: String,
    pub options: Vars,
}

impl Action {
    pub fn new(pid: &str, tid: &str, event: &str, options: &Vars) -> Self {
        Self {
            proc_id: pid.to_string(),
            task_id: tid.to_string(),
            event: event.to_string(),
            options: options.clone(),
        }
    }
}
