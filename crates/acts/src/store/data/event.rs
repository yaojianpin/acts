use crate::{
    Act, ActError, Result,
    store::{DbCollectionIden, StoreIden},
    utils,
};
use serde::{Deserialize, Serialize};

#[derive(Default, Deserialize, Serialize, Debug, Clone)]
pub struct Event {
    pub id: String,
    pub name: String,
    pub mid: String,
    pub ver: i32,

    pub uses: String,
    pub params: String,

    pub create_time: i64,
    pub timestamp: i64,
}

impl DbCollectionIden for Event {
    fn iden() -> StoreIden {
        StoreIden::Events
    }
}

impl Event {
    pub fn from_act(act: &Act, mid: &str, ver: i32, event_id: &str) -> Result<Self> {
        Ok(Self {
            id: event_id.to_string(),
            name: act.name.to_string(),
            mid: mid.to_string(),
            ver,
            uses: act.uses.clone(),
            params: serde_json::to_string(&act.params).map_err(|err| {
                ActError::Convert(format!("failed to convert params to string: {}", err))
            })?,
            create_time: utils::time::time_millis(),
            timestamp: utils::time::timestamp(),
        })
    }
}

#[cfg(test)]
mod tests {}
