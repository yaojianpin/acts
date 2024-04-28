use crate::{
    store::{db::mem::DbDocument, Task},
    Result,
};
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;

impl DbDocument for Task {
    fn id(&self) -> &str {
        &self.id
    }
    fn doc(&self) -> Result<HashMap<String, JsonValue>> {
        let mut map = HashMap::new();
        map.insert("id".to_string(), json!(self.id.clone()));
        map.insert("name".to_string(), json!(self.name.clone()));
        map.insert("proc_id".to_string(), json!(self.proc_id.clone()));
        map.insert("task_id".to_string(), json!(self.task_id.clone()));
        map.insert("node_id".to_string(), json!(self.node_id.clone()));
        map.insert("kind".to_string(), json!(self.kind.clone()));
        map.insert("prev".to_string(), json!(self.prev.clone()));
        map.insert("state".to_string(), json!(self.state.clone()));
        map.insert("data".to_string(), json!(self.data.clone()));
        map.insert("start_time".to_string(), json!(self.start_time));
        map.insert("end_time".to_string(), json!(self.end_time));
        map.insert("hooks".to_string(), json!(self.hooks.clone()));
        map.insert("timestamp".to_string(), json!(self.timestamp));
        Ok(map)
    }
}
