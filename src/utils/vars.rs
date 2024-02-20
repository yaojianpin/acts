use crate::Vars;
use serde_json::Value;

pub fn from_string(str: &str) -> Vars {
    let mut vars = Vars::new();
    if str.is_empty() {
        return vars;
    }
    let map: Value = serde_json::from_str(str).unwrap();
    let map = map.as_object().unwrap();
    for (k, v) in map {
        vars.insert(k.as_str().to_string(), v.clone());
    }

    vars
}
