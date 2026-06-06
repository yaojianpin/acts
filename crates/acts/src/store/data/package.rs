use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::{
    ActRunAs, Result,
    package::ActPackageCatalog,
    store::{DbCollectionIden, StoreIden},
};

#[derive(Default, Deserialize, Serialize, Debug, Clone)]
pub struct Package {
    pub id: String,
    pub name: String,
    pub desc: String,
    pub icon: String,
    pub doc: String,
    pub version: String,
    pub in_schema: String,
    pub ui_schema: Option<String>,
    pub run_as: ActRunAs,
    pub resources: String,
    pub catalog: ActPackageCatalog,
    pub built_in: bool,

    pub create_time: i64,
    pub update_time: i64,
    pub timestamp: i64,
    pub v: i32,
}

impl DbCollectionIden for Package {
    fn iden() -> StoreIden {
        StoreIden::Packages
    }

    fn indexed_fields() -> &'static [&'static str] {
        &["timestamp", "create_time", "update_time"]
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
            "unsupported package version: {}",
            v
        )))
    }
}
