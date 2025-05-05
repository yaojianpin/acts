use crate::{
    Result,
    store::{Event, db::mem::DbDocument},
};
use serde_json::{Value as JsonValue, json};
use std::collections::HashMap;

impl DbDocument for Event {
    fn id(&self) -> &str {
        &self.id
    }

    fn doc(&self) -> Result<HashMap<String, JsonValue>> {
        let mut map = HashMap::new();
        map.insert("id".to_string(), json!(self.id.clone()));
        map.insert("name".to_string(), json!(self.name.clone()));
        map.insert("mid".to_string(), json!(self.mid.clone()));

        map.insert("create_time".to_string(), json!(self.create_time));
        map.insert("timestamp".to_string(), json!(self.timestamp));
        Ok(map)
    }
}
