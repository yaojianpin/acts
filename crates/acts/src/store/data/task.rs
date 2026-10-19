use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::{
    LegacyTaskState, Result,
    store::{DbCollectionIden, StoreIden},
};

/// The lifecycle row of one task instance: identity, node link graph, state
/// and timing. Scope variables live in the paired [`TaskVars`] row so state
/// transitions never rewrite variable bytes.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
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
    /// The aggregate version this row was projected from (A5), in the
    /// `"<graph>.<process>.<operations>"` encoding of `ProcessVersion`.
    /// Missing on pre-A5 rows (decoded as an empty string, older than every
    /// aggregate).
    #[serde(default)]
    pub source_revision: String,
    pub v: i32,
}

impl DbCollectionIden for Task {
    fn iden() -> String {
        StoreIden::Tasks.as_ref().to_string()
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
    pub fn set_state(&mut self, state: LegacyTaskState) {
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
/// task's own variable scope. `sealed` stays in the fixed row shape but the
/// kernel writes it empty: snapshot-backed sealed data is resolved per effect
/// and is not durable run state. Rows are written when a scope's vars actually
/// change and are dropped with the process.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct TaskVars {
    pub id: String,
    pub pid: String,
    pub tid: String,
    #[serde(default)]
    pub data: String,
    /// Retained for the fixed `vars` row shape and pre-kernel rows; the
    /// projection always writes an empty string. A snapshot seal is the
    /// effect seam's per-evaluation authorization/data resolution, not a
    /// recoverable run state, and no read API (`proc:get`, the read model)
    /// consumes this field.
    #[serde(default)]
    pub sealed: String,
    /// The aggregate version this row was projected from (A5); see
    /// [`Task::source_revision`].
    #[serde(default)]
    pub source_revision: String,
    pub v: i32,
}

impl DbCollectionIden for TaskVars {
    fn iden() -> String {
        StoreIden::Vars.as_ref().to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn task() -> Task {
        Task {
            id: "p1-t1".into(),
            pid: "p1".into(),
            tid: "t1".into(),
            node_data: "{}".into(),
            kind: "step".into(),
            prev: None,
            next: Vec::new(),
            parent: None,
            name: "test".into(),
            state: "running".into(),
            err: None,
            start_time: 0,
            end_time: 0,
            timestamp: 1000,
            source_revision: "g.p.0".into(),
            v: Task::version(),
        }
    }

    #[test]
    fn task_metadata_and_state_setters() {
        assert_eq!(Task::iden(), "tasks");
        assert_eq!(
            Task::ordered_index_fields(),
            &["timestamp", "start_time", "end_time"]
        );
        assert_eq!(Task::version(), 0);

        let mut row = task();
        row.set_state(LegacyTaskState::Completed);
        assert_eq!(row.state, "completed");
        row.set_start_time(10);
        row.set_end_time(20);
        assert_eq!((row.start_time, row.end_time), (10, 20));
    }

    #[test]
    fn task_upcast_current_and_missing_version() {
        let json = serde_json::to_value(task()).unwrap();
        assert_eq!(Task::upcast(json).unwrap().id, "p1-t1");

        let mut json = serde_json::to_value(task()).unwrap();
        json.as_object_mut().unwrap().remove("v");
        let restored = Task::upcast(json).unwrap();
        assert_eq!(restored.v, 0, "missing v decodes as the oldest version");
    }

    #[test]
    fn task_upcast_rejects_unknown_version() {
        let mut json = serde_json::to_value(task()).unwrap();
        json["v"] = serde_json::json!(42);
        let err = Task::upcast(json).unwrap_err();
        assert!(err.to_string().contains("unsupported task version: 42"));
    }

    #[test]
    fn task_vars_upcast_current_and_rejects_unknown_version() {
        assert_eq!(TaskVars::iden(), "vars");
        assert_eq!(TaskVars::version(), 0);

        let vars = TaskVars {
            id: "p1-t1".into(),
            pid: "p1".into(),
            tid: "t1".into(),
            data: "{}".into(),
            sealed: String::new(),
            source_revision: String::new(),
            v: TaskVars::version(),
        };
        let json = serde_json::to_value(&vars).unwrap();
        assert_eq!(TaskVars::upcast(json).unwrap().id, "p1-t1");

        let err = TaskVars::upcast(serde_json::json!({"id": "x", "v": 9})).unwrap_err();
        assert!(err.to_string().contains("unsupported task vars version: 9"));
    }
}
