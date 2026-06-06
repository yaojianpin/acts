use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::{
    Act, ActError, Result,
    store::{DbCollectionIden, StoreIden},
    utils,
};

#[derive(Default, Deserialize, Serialize, Debug, Clone)]
pub struct Event {
    pub id: String,
    pub name: String,
    pub mid: String,
    pub ver: String,

    pub uses: String,
    pub params: String,

    pub create_time: i64,
    pub timestamp: i64,
    pub v: i32,
}

impl DbCollectionIden for Event {
    fn iden() -> StoreIden {
        StoreIden::Events
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
            "unsupported event version: {}",
            v
        )))
    }
}

impl Event {
    pub fn from_act(act: &Act, mid: &str, ver: &str, event_id: &str) -> Result<Self> {
        Ok(Self {
            id: event_id.to_string(),
            name: act.name.to_string(),
            mid: mid.to_string(),
            ver: ver.to_string(),
            uses: act.uses.clone(),
            params: serde_json::to_string(&act.params).map_err(|err| {
                ActError::Convert(format!("failed to convert params to string: {err}"))
            })?,
            create_time: utils::time::time_millis(),
            timestamp: utils::time::timestamp(),
            v: Self::version(),
        })
    }
}
