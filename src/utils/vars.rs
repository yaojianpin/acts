use crate::Vars;
use serde_yaml::Value;
use std::collections::HashMap;

pub fn to_string(vars: &Vars) -> String {
    serde_yaml::to_string(vars).unwrap()
}

pub fn from_string(str: &str) -> Vars {
    let mut vars = HashMap::new();
    if str.is_empty() {
        return vars;
    }
    let map: Value = serde_yaml::from_str(str).unwrap();
    let map = map.as_mapping().unwrap();
    for (k, v) in map {
        vars.insert(k.as_str().unwrap().to_string(), v.clone());
    }

    vars
}
