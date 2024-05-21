use crate::{
    store::{db::mem::DbDocument, Message},
    Result,
};
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;

impl DbDocument for Message {
    fn id(&self) -> &str {
        &self.id
    }

    fn doc(&self) -> Result<HashMap<String, JsonValue>> {
        let mut map = HashMap::new();
        map.insert("id".to_string(), json!(self.id.clone()));
        map.insert("name".to_string(), json!(self.name.clone()));
        map.insert("tid".to_string(), json!(self.tid.clone()));
        map.insert("state".to_string(), json!(self.state.clone()));
        map.insert("type".to_string(), json!(self.r#type.clone()));
        map.insert("source".to_string(), json!(self.source.clone()));
        map.insert("model".to_string(), json!(self.model.clone()));
        map.insert("pid".to_string(), json!(self.pid.clone()));
        map.insert("key".to_string(), json!(self.key.clone()));
        map.insert("inputs".to_string(), json!(self.inputs.clone()));
        map.insert("outputs".to_string(), json!(self.outputs.clone()));
        map.insert("tag".to_string(), json!(self.tag.clone()));
        map.insert("start_time".to_string(), json!(self.start_time));
        map.insert("end_time".to_string(), json!(self.end_time));
        map.insert("chan_id".to_string(), json!(self.chan_id.clone()));
        map.insert("chan_pattern".to_string(), json!(self.chan_pattern));
        map.insert("create_time".to_string(), json!(self.create_time));
        map.insert("update_time".to_string(), json!(self.update_time));
        map.insert("status".to_string(), json!(self.status));
        map.insert("retry_times".to_string(), json!(self.retry_times));
        map.insert("timestamp".to_string(), json!(self.timestamp));
        Ok(map)
    }
}
