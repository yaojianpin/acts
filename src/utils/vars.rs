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

pub fn from_json(map: &serde_json::Map<String, serde_json::Value>) -> Vars {
    let mut vars = Vars::new();

    for (k, v) in map {
        let value = match v {
            serde_json::Value::Null => Value::Null,
            serde_json::Value::Bool(v) => Value::Bool(v.clone()),
            serde_json::Value::Number(v) => from_json_number(v),
            serde_json::Value::String(v) => Value::String(v.clone()),
            serde_json::Value::Array(v) => from_json_array(v),
            serde_json::Value::Object(v) => from_json_object(v),
        };

        vars.insert(k.to_string(), value);
    }

    vars
}

fn from_json_array(arr: &Vec<serde_json::Value>) -> serde_yaml::Value {
    let mut ret = Vec::new();
    for v in arr {
        let value = match v {
            serde_json::Value::Null => Value::Null,
            serde_json::Value::Bool(v) => Value::Bool(v.clone()),
            serde_json::Value::Number(v) => from_json_number(v),
            serde_json::Value::String(v) => Value::String(v.clone()),
            serde_json::Value::Array(v) => from_json_array(v),
            serde_json::Value::Object(v) => from_json_object(v),
        };
        ret.push(value);
    }

    Value::Sequence(ret)
}

fn from_json_object(o: &serde_json::Map<String, serde_json::Value>) -> Value {
    let mut map = serde_yaml::Mapping::new();
    for (k, v) in o {
        let value = match v {
            serde_json::Value::Null => Value::Null,
            serde_json::Value::Bool(v) => Value::Bool(v.clone()),
            serde_json::Value::Number(v) => from_json_number(v),
            serde_json::Value::String(v) => Value::String(v.clone()),
            serde_json::Value::Array(v) => from_json_array(v),
            serde_json::Value::Object(v) => from_json_object(v),
        };

        map.insert(Value::String(k.to_string()), value);
    }

    Value::Mapping(map)
}

fn from_json_number(n: &serde_json::Number) -> Value {
    if n.is_i64() {
        return Value::Number(serde_yaml::Number::from(n.as_i64().unwrap()));
    } else if n.is_u64() {
        return Value::Number(serde_yaml::Number::from(n.as_u64().unwrap()));
    } else {
        return Value::Number(serde_yaml::Number::from(n.as_f64().unwrap()));
    }
}
