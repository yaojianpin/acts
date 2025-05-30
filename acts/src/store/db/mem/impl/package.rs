use crate::{
    Result,
    store::{Package, db::mem::DbDocument},
};
use serde_json::{Value as JsonValue, json};
use std::collections::HashMap;

impl DbDocument for Package {
    fn id(&self) -> &str {
        &self.id
    }

    fn doc(&self) -> Result<HashMap<String, JsonValue>> {
        let mut map = HashMap::new();
        map.insert("id".to_string(), json!(self.id.clone()));

        map.insert("desc".to_string(), json!(self.desc.clone()));
        map.insert("icon".to_string(), json!(self.icon.clone()));
        map.insert("doc".to_string(), json!(self.doc.clone()));
        map.insert("version".to_string(), json!(self.version.clone()));
        map.insert("schema".to_string(), json!(self.schema.clone()));
        map.insert("run_as".to_string(), json!(self.run_as.clone()));
        map.insert("resources".to_string(), json!(self.resources.clone()));
        map.insert("catalog".to_string(), json!(self.catalog.clone()));

        map.insert("create_time".to_string(), json!(self.create_time.clone()));
        map.insert("update_time".to_string(), json!(self.update_time.clone()));
        map.insert("timestamp".to_string(), json!(self.timestamp.clone()));
        map.insert("built_in".to_string(), json!(self.built_in));
        Ok(map)
    }
}
