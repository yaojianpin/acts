use crate::{
    Result,
    store::{Model, db::mem::DbDocument},
};
use serde_json::{Value as JsonValue, json};
use std::collections::HashMap;

impl DbDocument for Model {
    fn id(&self) -> &str {
        &self.id
    }

    fn doc(&self) -> Result<HashMap<String, JsonValue>> {
        let mut map = HashMap::new();
        map.insert("id".to_string(), json!(self.id.clone()));
        map.insert("name".to_string(), json!(self.name.clone()));
        map.insert("ver".to_string(), json!(self.ver));
        map.insert("size".to_string(), json!(self.size));
        map.insert("create_time".to_string(), json!(self.create_time));
        map.insert("update_time".to_string(), json!(self.update_time));
        map.insert("data".to_string(), json!(self.data.clone()));
        map.insert("timestamp".to_string(), json!(self.timestamp));
        Ok(map)
    }
}
