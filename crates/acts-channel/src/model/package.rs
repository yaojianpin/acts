use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct Package {
    pub id: String,
    pub name: String,
    pub desc: String,
    pub icon: String,
    pub doc: String,
    pub version: String,
    pub schema: serde_json::Value,
    pub options: Option<serde_json::Value>,
    pub run_as: String,
    pub resources: Vec<serde_json::Value>,
    pub catalog: String,
}
