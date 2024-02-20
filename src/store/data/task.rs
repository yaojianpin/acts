use super::DbModel;
use crate::TaskState;
use acts_tag::{Tags, Value};
use serde::{Deserialize, Serialize};

#[derive(Tags, Debug, Deserialize, Serialize, Clone)]
pub struct Task {
    #[tag(id)]
    pub id: String,
    #[tag]
    pub proc_id: String,
    #[tag]
    pub task_id: String,
    #[tag]
    pub node_id: String,
    #[tag]
    pub kind: String,
    #[tag]
    pub prev: Option<String>,

    pub name: String,
    pub state: String,
    pub action_state: String,
    pub start_time: i64,
    pub end_time: i64,
    pub hooks: String,
    pub timestamp: i64,
}

impl Task {
    pub fn set_state(&mut self, state: TaskState) {
        self.state = state.into();
    }
    pub fn set_start_time(&mut self, time: i64) {
        self.start_time = time;
    }
    pub fn set_end_time(&mut self, time: i64) {
        self.end_time = time;
    }
}

impl DbModel for Task {
    fn id(&self) -> &str {
        &self.id
    }
}
