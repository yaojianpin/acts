use crate::{
    store::{db::mem::DbDocument, Package},
    Result,
};
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;

impl DbDocument for Package {
    fn id(&self) -> &str {
        &self.id
    }

    fn doc(&self) -> Result<HashMap<String, JsonValue>> {
        let mut map = HashMap::new();
        map.insert("id".to_string(), json!(self.id.clone()));
        map.insert("name".to_string(), json!(self.name.clone()));
        map.insert("size".to_string(), json!(self.size.clone()));
        map.insert(
            "file_data".to_string(),
            JsonValue::String(hex::encode(&self.file_data)),
        );
        map.insert("create_time".to_string(), json!(self.create_time.clone()));
        map.insert("update_time".to_string(), json!(self.update_time.clone()));
        map.insert("timestamp".to_string(), json!(self.timestamp.clone()));
        Ok(map)
    }
}
