use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::Result;
use crate::store::{DbCollectionIden, StoreIden};

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub desc: String,
    pub ver: String,
    pub size: i32,
    pub create_time: i64,
    pub update_time: i64,
    pub data: String,
    pub view: Option<String>,
    pub timestamp: i64,
    pub v: i32,
}

impl DbCollectionIden for Model {
    fn iden() -> StoreIden {
        StoreIden::Models
    }

    fn indexed_fields() -> &'static [&'static str] {
        &["name", "timestamp", "create_time", "update_time", "ver"]
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
            "unsupported model version: {}",
            v
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_version_returns_current() {
        assert_eq!(Model::version(), 0);
    }

    #[test]
    fn model_upcast_with_v0_works() {
        let mut map = serde_json::Map::new();
        map.insert("id".to_string(), JsonValue::String("m1".to_string()));
        map.insert("name".to_string(), JsonValue::String("test".to_string()));
        map.insert("desc".to_string(), JsonValue::String("desc".to_string()));
        map.insert("ver".to_string(), JsonValue::String("0.1.0".to_string()));
        map.insert(
            "size".to_string(),
            JsonValue::Number(serde_json::Number::from(100)),
        );
        map.insert(
            "create_time".to_string(),
            JsonValue::Number(serde_json::Number::from(500)),
        );
        map.insert(
            "update_time".to_string(),
            JsonValue::Number(serde_json::Number::from(0)),
        );
        map.insert("data".to_string(), JsonValue::String("{}".to_string()));
        map.insert(
            "timestamp".to_string(),
            JsonValue::Number(serde_json::Number::from(1000)),
        );
        map.insert(
            "v".to_string(),
            JsonValue::Number(serde_json::Number::from(0)),
        );

        let model = Model::upcast(JsonValue::Object(map)).unwrap();
        assert_eq!(model.id, "m1");
        assert_eq!(model.name, "test");
        assert_eq!(model.v, 0);
    }

    #[test]
    fn model_upcast_missing_v_defaults_to_0() {
        let mut map = serde_json::Map::new();
        map.insert("id".to_string(), JsonValue::String("m2".to_string()));
        map.insert("name".to_string(), JsonValue::String("test".to_string()));
        map.insert("desc".to_string(), JsonValue::String("desc".to_string()));
        map.insert("ver".to_string(), JsonValue::String("0.1.0".to_string()));
        map.insert(
            "size".to_string(),
            JsonValue::Number(serde_json::Number::from(100)),
        );
        map.insert(
            "create_time".to_string(),
            JsonValue::Number(serde_json::Number::from(500)),
        );
        map.insert(
            "update_time".to_string(),
            JsonValue::Number(serde_json::Number::from(0)),
        );
        map.insert("data".to_string(), JsonValue::String("{}".to_string()));
        map.insert(
            "timestamp".to_string(),
            JsonValue::Number(serde_json::Number::from(1000)),
        );
        // v field is intentionally missing

        let model = Model::upcast(JsonValue::Object(map)).unwrap();
        assert_eq!(model.id, "m2");
        assert_eq!(model.v, 0); // defaults to 0
    }

    #[test]
    fn model_upcast_unknown_version_fails() {
        let mut map = serde_json::Map::new();
        map.insert("id".to_string(), JsonValue::String("m3".to_string()));
        map.insert("name".to_string(), JsonValue::String("test".to_string()));
        map.insert("desc".to_string(), JsonValue::String("desc".to_string()));
        map.insert("ver".to_string(), JsonValue::String("0.1.0".to_string()));
        map.insert(
            "size".to_string(),
            JsonValue::Number(serde_json::Number::from(100)),
        );
        map.insert(
            "create_time".to_string(),
            JsonValue::Number(serde_json::Number::from(500)),
        );
        map.insert(
            "update_time".to_string(),
            JsonValue::Number(serde_json::Number::from(0)),
        );
        map.insert("data".to_string(), JsonValue::String("{}".to_string()));
        map.insert(
            "timestamp".to_string(),
            JsonValue::Number(serde_json::Number::from(1000)),
        );
        map.insert(
            "v".to_string(),
            JsonValue::Number(serde_json::Number::from(99)),
        );

        let result = Model::upcast(JsonValue::Object(map));
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unsupported model version: 99")
        );
    }
}
