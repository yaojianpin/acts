use crate::TaskState;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Task {
    pub id: String,
    pub kind: String,
    pub pid: String,
    pub tid: String,
    pub nid: String,
    pub state: String,
    pub start_time: i64,
    pub end_time: i64,
    pub uid: String,
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
    pub fn set_user(&mut self, user: &str) {
        self.uid = user.to_string();
    }
}
