use crate::{
    store::{db::mem::DbDocument, Model},
    Result,
};
use serde_json::{json, Value as JsonValue};
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
        map.insert("time".to_string(), json!(self.time));
        map.insert("data".to_string(), json!(self.data.clone()));
        Ok(map)
    }
}
