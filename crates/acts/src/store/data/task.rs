use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::{
    Result, TaskState,
    store::{DbCollectionIden, StoreIden},
};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Task {
    pub id: String,
    pub pid: String,
    pub tid: String,
    pub node_data: String,
    pub kind: String,
    pub prev: Option<String>,
    pub next: Vec<String>,
    pub parent: Option<String>,

    pub name: String,
    pub state: String,
    pub data: String,
    pub err: Option<String>,
    pub start_time: i64,
    pub end_time: i64,
    pub timestamp: i64,
    pub v: i32,
}

impl DbCollectionIden for Task {
    fn iden() -> StoreIden {
        StoreIden::Tasks
    }
    fn indexed_fields() -> &'static [&'static str] {
        &["pid", "tid", "state", "timestamp", "start_time", "end_time"]
    }
    fn version() -> i32 {
        0
    }

    fn upcast(value: JsonValue) -> Result<Self> {
        let v = value.get("v").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        if v == Self::version() {
            return Self::upcast_current(value);
        }
        Err(crate::ActError::Store(format!(
            "unsupported task version: {}",
            v
        )))
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
