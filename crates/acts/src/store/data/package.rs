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
    pub schema: String,
    pub options: Option<String>,
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
    fn iden() -> String {
        StoreIden::Packages.as_ref().to_string()
    }

    fn indexed_fields() -> &'static [&'static str] {
        &["timestamp", "create_time", "update_time"]
    }
    fn ordered_index_fields() -> &'static [&'static str] {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_metadata_and_current_upcast() {
        assert_eq!(Package::iden(), "packages");
        assert_eq!(
            Package::ordered_index_fields(),
            &["timestamp", "create_time", "update_time"]
        );
        assert_eq!(Package::version(), 0);

        let row = Package {
            id: "p1".into(),
            ..Default::default()
        };
        let json = serde_json::to_value(&row).unwrap();
        let restored = Package::upcast(json).unwrap();
        assert_eq!(restored.id, "p1");
    }

    #[test]
    fn package_upcast_accepts_missing_version_and_rejects_unknown() {
        let row = Package {
            id: "p2".into(),
            ..Default::default()
        };
        let mut json = serde_json::to_value(&row).unwrap();
        json.as_object_mut().unwrap().remove("v");
        assert_eq!(Package::upcast(json).unwrap().id, "p2");

        let json = serde_json::json!({"id": "p3", "v": 7});
        let err = Package::upcast(json).unwrap_err();
        assert!(err.to_string().contains("unsupported package version: 7"));
    }
}
