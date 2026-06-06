use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::Result;
use crate::store::{DbCollectionIden, StoreIden};

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Proc {
    pub id: String,
    pub state: String,
    pub mid: String,
    pub name: String,
    pub start_time: i64,
    pub end_time: i64,
    pub timestamp: i64,
    pub model: String,
    pub env: String,
    pub err: Option<String>,
    pub v: i32,
}

impl DbCollectionIden for Proc {
    fn iden() -> StoreIden {
        StoreIden::Procs
    }
    fn indexed_fields() -> &'static [&'static str] {
        &["state", "mid", "timestamp", "start_time", "end_time"]
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
            "unsupported proc version: {}",
            v
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_version_returns_current() {
        assert_eq!(Proc::version(), 0);
    }

    #[test]
    fn proc_upcast_with_v0_works() {
        let mut map = serde_json::Map::new();
        map.insert("id".to_string(), JsonValue::String("p1".to_string()));
        map.insert(
            "state".to_string(),
            JsonValue::String("running".to_string()),
        );
        map.insert("mid".to_string(), JsonValue::String("m1".to_string()));
        map.insert("name".to_string(), JsonValue::String("test".to_string()));
        map.insert(
            "start_time".to_string(),
            JsonValue::Number(serde_json::Number::from(0)),
        );
        map.insert(
            "end_time".to_string(),
            JsonValue::Number(serde_json::Number::from(0)),
        );
        map.insert(
            "timestamp".to_string(),
            JsonValue::Number(serde_json::Number::from(1000)),
        );
        map.insert("model".to_string(), JsonValue::String("{}".to_string()));
        map.insert("env".to_string(), JsonValue::String("{}".to_string()));
        map.insert("err".to_string(), JsonValue::Null);
        map.insert(
            "v".to_string(),
            JsonValue::Number(serde_json::Number::from(0)),
        );

        let proc = Proc::upcast(JsonValue::Object(map)).unwrap();
        assert_eq!(proc.id, "p1");
        assert_eq!(proc.state, "running");
        assert_eq!(proc.v, 0);
    }

    #[test]
    fn proc_upcast_missing_v_defaults_to_0() {
        let mut map = serde_json::Map::new();
        map.insert("id".to_string(), JsonValue::String("p2".to_string()));
        map.insert(
            "state".to_string(),
            JsonValue::String("completed".to_string()),
        );
        map.insert("mid".to_string(), JsonValue::String("m2".to_string()));
        map.insert("name".to_string(), JsonValue::String("test".to_string()));
        map.insert(
            "start_time".to_string(),
            JsonValue::Number(serde_json::Number::from(0)),
        );
        map.insert(
            "end_time".to_string(),
            JsonValue::Number(serde_json::Number::from(0)),
        );
        map.insert(
            "timestamp".to_string(),
            JsonValue::Number(serde_json::Number::from(1000)),
        );
        map.insert("model".to_string(), JsonValue::String("{}".to_string()));
        map.insert("env".to_string(), JsonValue::String("{}".to_string()));
        map.insert("err".to_string(), JsonValue::Null);
        // v field intentionally missing

        let proc = Proc::upcast(JsonValue::Object(map)).unwrap();
        assert_eq!(proc.id, "p2");
        assert_eq!(proc.v, 0);
    }

    #[test]
    fn proc_upcast_unknown_version_fails() {
        let mut map = serde_json::Map::new();
        map.insert("id".to_string(), JsonValue::String("p3".to_string()));
        map.insert(
            "state".to_string(),
            JsonValue::String("running".to_string()),
        );
        map.insert("mid".to_string(), JsonValue::String("m3".to_string()));
        map.insert("name".to_string(), JsonValue::String("test".to_string()));
        map.insert(
            "start_time".to_string(),
            JsonValue::Number(serde_json::Number::from(0)),
        );
        map.insert(
            "end_time".to_string(),
            JsonValue::Number(serde_json::Number::from(0)),
        );
        map.insert(
            "timestamp".to_string(),
            JsonValue::Number(serde_json::Number::from(1000)),
        );
        map.insert("model".to_string(), JsonValue::String("{}".to_string()));
        map.insert("env".to_string(), JsonValue::String("{}".to_string()));
        map.insert("err".to_string(), JsonValue::Null);
        map.insert(
            "v".to_string(),
            JsonValue::Number(serde_json::Number::from(99)),
        );

        let result = Proc::upcast(JsonValue::Object(map));
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unsupported proc version: 99")
        );
    }
}
