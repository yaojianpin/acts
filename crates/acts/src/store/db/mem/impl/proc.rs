use crate::{
    Result,
    store::{Proc, db::mem::DbDocument},
};
use serde_json::{Value as JsonValue, json};
use std::collections::HashMap;

impl DbDocument for Proc {
    fn id(&self) -> &str {
        &self.id
    }

    fn doc(&self) -> Result<HashMap<String, JsonValue>> {
        let mut map = HashMap::new();
        map.insert("id".to_string(), json!(self.id.clone()));
        map.insert("name".to_string(), json!(self.name.clone()));
        map.insert("state".to_string(), json!(self.state.clone()));
        map.insert("mid".to_string(), json!(self.mid.clone()));
        map.insert("start_time".to_string(), json!(self.start_time));
        map.insert("end_time".to_string(), json!(self.end_time));
        map.insert("timestamp".to_string(), json!(self.timestamp));
        map.insert("model".to_string(), json!(self.model.clone()));
        map.insert("env".to_string(), json!(self.env.clone()));
        Ok(map)
    }
}
