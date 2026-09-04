use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::{
    Result, Trigger,
    store::{DbCollectionIden, StoreIden},
    utils,
};

#[derive(Default, Deserialize, Serialize, Debug, Clone)]
pub struct Event {
    pub id: String,
    pub name: String,
    pub mid: String,
    pub ver: String,

    /// trigger kind: `manual`/`chat`/`hook`/`schedule`, or a
    /// registered event package id for custom kinds
    pub kind: String,
    /// default start inputs (trigger params) as json text
    pub params: String,
    /// kind=`schedule`: cron expression
    #[serde(default)]
    pub schedule: Option<String>,

    /// kind=`schedule`: last run time in millis (0 = never)
    #[serde(default)]
    pub last_run: i64,
    /// kind=`schedule`: next run time in millis; 0 arms it on the next tick
    #[serde(default)]
    pub next_run: i64,

    pub create_time: i64,
    pub timestamp: i64,
    pub v: i32,
}

impl DbCollectionIden for Event {
    fn iden() -> StoreIden {
        StoreIden::Events
    }
    fn version() -> i32 {
        1
    }

    fn upcast(value: JsonValue) -> Result<Self> {
        let v = value.get("v").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        match v {
            0 => {
                // v0 rows carried `uses: acts.event.X` + params. Migrate the
                // built-in kinds to `kind`; unknown custom uses are kept as
                // the kind so the package fire path still resolves them.
                let mut value = value;
                if let Some(map) = value.as_object_mut() {
                    let uses = map
                        .remove("uses")
                        .and_then(|v| v.as_str().map(str::to_string));
                    if let Some(uses) = uses {
                        let kind = match uses.as_str() {
                            "acts.event.manual" => "manual".to_string(),
                            "acts.event.hook" => "hook".to_string(),
                            "acts.event.chat" => "chat".to_string(),
                            _ => uses,
                        };
                        map.insert("kind".to_string(), JsonValue::String(kind));
                    }
                }
                value.as_object_mut().map(|map| {
                    map.insert(
                        "v".to_string(),
                        JsonValue::Number(serde_json::Number::from(Self::version())),
                    )
                });
                Self::upcast_current(value)
            }
            _ if v == Self::version() => Self::upcast_current(value),
            _ => Err(crate::ActError::Store(format!(
                "unsupported event version: {}",
                v
            ))),
        }
    }
}

impl Event {
    pub fn from_trigger(trigger: &Trigger, mid: &str, ver: &str, event_id: &str) -> Result<Self> {
        Ok(Self {
            id: event_id.to_string(),
            name: trigger.name.clone(),
            mid: mid.to_string(),
            ver: ver.to_string(),
            kind: trigger.kind.clone(),
            params: serde_json::to_string(&trigger.params).map_err(|err| {
                crate::ActError::Convert(format!("failed to convert params to string: {err}"))
            })?,
            schedule: trigger.schedule.clone(),
            last_run: 0,
            next_run: 0,
            create_time: utils::time::time_millis(),
            timestamp: utils::time::timestamp(),
            v: Self::version(),
        })
    }

    pub fn default_params(&self) -> JsonValue {
        serde_json::from_str(&self.params).unwrap_or(JsonValue::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn event_upcast_v0_builtin_uses_maps_to_kind() {
        let doc = json!({
            "id": "m:e1",
            "name": "",
            "mid": "m",
            "ver": "0.1.0",
            "uses": "acts.event.manual",
            "params": "{\"test\":10}",
            "create_time": 0,
            "timestamp": 0,
        });
        let evt = Event::upcast(doc).unwrap();
        assert_eq!(evt.kind, "manual");
        assert_eq!(evt.v, 1);
        assert_eq!(evt.schedule, None);
        assert_eq!(evt.last_run, 0);
        assert_eq!(evt.next_run, 0);
    }

    #[test]
    fn event_upcast_v0_custom_uses_kept_as_kind() {
        let doc = json!({
            "id": "m:e1",
            "name": "",
            "mid": "m",
            "ver": "0.1.0",
            "uses": "my.custom.trigger",
            "params": "{}",
            "create_time": 0,
            "timestamp": 0,
        });
        let evt = Event::upcast(doc).unwrap();
        assert_eq!(evt.kind, "my.custom.trigger");
    }

    #[test]
    fn event_upcast_current_version_ok() {
        let doc = json!({
            "id": "m:e1",
            "name": "",
            "mid": "m",
            "ver": "0.1.0",
            "kind": "schedule",
            "params": "{}",
            "schedule": "* * * * * *",
            "last_run": 0,
            "next_run": 100,
            "create_time": 0,
            "timestamp": 0,
            "v": 1,
        });
        let evt = Event::upcast(doc).unwrap();
        assert_eq!(evt.kind, "schedule");
        assert_eq!(evt.next_run, 100);
    }

    #[test]
    fn event_upcast_unknown_version_err() {
        let doc = json!({ "id": "m:e1", "v": 9 });
        assert!(Event::upcast(doc).is_err());
    }
}
