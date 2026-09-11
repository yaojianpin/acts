use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::{
    Result, TaskState,
    store::{DbCollectionIden, StoreIden},
};

/// The lifecycle row of one task instance: identity, node link graph, state
/// and timing. Scope variables live in the paired [`TaskVars`] row so state
/// transitions never rewrite variable bytes.
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
    fn ordered_index_fields() -> &'static [&'static str] {
        &["timestamp", "start_time", "end_time"]
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

/// Scope vars of one task, stored apart from the task's lifecycle row so
/// state transitions never rewrite variable bytes (and a variable write
/// never needs to touch another task's row). One row per task scope, keyed by
/// the same composite `pid + tid` id as its [`Task`] row; `data` holds the
/// task's own variable scope, `sealed` the resolver-written sealed scope.
/// Rows are written when a scope's vars actually change and are dropped with
/// the process.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TaskVars {
    pub id: String,
    pub pid: String,
    pub tid: String,
    #[serde(default)]
    pub data: String,
    #[serde(default)]
    pub sealed: String,
    pub v: i32,
}

impl DbCollectionIden for TaskVars {
    fn iden() -> StoreIden {
        StoreIden::Vars
    }
    fn indexed_fields() -> &'static [&'static str] {
        &["pid", "tid"]
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
            "unsupported task vars version: {}",
            v
        )))
    }
}
