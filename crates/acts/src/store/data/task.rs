use crate::{
    TaskState,
    store::{DbCollectionIden, StoreIden},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Task {
    pub id: String,
    pub pid: String,
    pub tid: String,
    pub node_data: String,
    pub kind: String,
    pub prev: Option<String>,

    pub name: String,
    pub state: String,
    pub data: String,
    pub err: Option<String>,
    pub start_time: i64,
    pub end_time: i64,
    pub hooks: String,
    pub timestamp: i64,
}

impl DbCollectionIden for Task {
    fn iden() -> StoreIden {
        StoreIden::Tasks
    }
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
