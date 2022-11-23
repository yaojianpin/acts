use crate::env::VirtualMachine;
use chrono::{DateTime, Local, TimeZone};
use regex::Regex;
use rhai::{Dynamic, Map};
use serde_yaml::{Mapping, Value};
use std::collections::HashMap;

pub fn value_to_dymainc(v: &Value) -> Dynamic {
    match v {
        Value::Null => Dynamic::UNIT,
        Value::Bool(b) => Dynamic::from(b.clone()),
        Value::String(s) => Dynamic::from(s.clone()),
        Value::Number(n) if n.is_i64() => Dynamic::from(n.as_i64().unwrap()),
        Value::Number(n) if n.is_f64() => Dynamic::from(n.as_f64().unwrap()),
        Value::Sequence(s) => Dynamic::from(array_to_dynamic(&s.clone())),
        Value::Mapping(m) => Dynamic::from(map_to_dynamic(m.clone())),
        _ => Dynamic::default(),
    }
}

pub fn dynamic_to_value(value: &Dynamic) -> Value {
    if value.is::<rhai::INT>() {
        let int = value.as_int().unwrap() as i64;
        return int.into();
    } else if value.is::<rhai::FLOAT>() {
        let float = value.as_float().unwrap() as f64;
        return float.into();
    } else if value.is::<bool>() {
        let b = value.as_bool().unwrap();
        return b.into();
    } else if value.is::<rhai::ImmutableString>() {
        let s = value.clone().into_string().unwrap();
        return s.into();
    }

    Value::Null
}

// pub fn dynamics_to_value(values: &Vec<Dynamic>) -> Value {
//     let mut ret = Vec::new();
//     for v in values {
//         ret.push(dynamic_to_value(v));
//     }

//     Value::Sequence(ret)
// }

pub fn array_to_dynamic<'a>(values: &'a Vec<Value>) -> Vec<Dynamic> {
    let mut ret = Vec::new();

    for v in values {
        ret.push(value_to_dymainc(v));
    }

    ret
}

pub fn map_to_dynamic<'a>(map: Mapping) -> Map {
    let mut ret: Map = Map::new();
    for (k, v) in &map {
        let key = k.as_str().unwrap().to_string();
        ret.insert(key.into(), value_to_dymainc(v));
    }

    ret
}

// pub fn ymap_to_act<'a>(
//     vm: &VirtualMachine,
//     values: &'a HashMap<String, Value>,
// ) -> HashMap<String, ActValue> {
//     let mut ret = HashMap::new();

//     for (k, v) in values {
//         let value: ActValue = v.into();
//         if let ActValue::Expr(expr) = value {
//             let result = vm.eval::<Dynamic>(&expr);
//             let new_value = match result {
//                 Ok(v) => v.into(),
//                 Err(_err) => ActValue::Null,
//             };

//             ret.insert(k.to_string(), new_value);
//         } else {
//             ret.insert(k.to_string(), value);
//         }
//     }

//     ret
// }

/// fill the vars
/// 1. if the outputs is an expression, just calculate it
/// 2. if the env and the outpus both has the same key, using the env to replace the one in outputs
pub fn fill_vars<'a>(
    vm: &VirtualMachine,
    values: &'a HashMap<String, Value>,
) -> HashMap<String, Value> {
    let mut ret = HashMap::new();

    for (k, v) in values {
        if let Value::String(string) = v {
            if let Some(expr) = get_expr(string) {
                let result = vm.eval::<Dynamic>(&expr);
                let new_value = match result {
                    Ok(v) => dynamic_to_value(&v),
                    Err(_err) => Value::Null,
                };

                // satisfies the rule 1
                ret.insert(k.to_string(), new_value);
                continue;
            }
        }

        // rule 2
        match vm.get(k) {
            Some(env_value) => ret.insert(k.to_string(), env_value),
            None => ret.insert(k.to_string(), v.clone()),
        };
    }

    ret
}

pub fn get_expr(text: &str) -> Option<String> {
    let re = Regex::new(r"^\$\{(.+)\}$").unwrap();
    let caps = re.captures(&text);

    if let Some(caps) = caps {
        let value = caps.get(1).map_or("", |m| m.as_str());
        return Some(value.trim().to_string());
    }

    None
}

pub fn fmt_timestamp(
    start_time_mills: i64,
    end_time_mills: i64,
    fmt: &str,
) -> (String, String, i64) {
    let start_time = parse_timestamp(start_time_mills);
    let end_time = parse_timestamp(end_time_mills);

    let elapsed = end_time_mills - start_time_mills;

    (
        start_time.format(fmt).to_string(),
        end_time.format(fmt).to_string(),
        elapsed,
    )
}

fn parse_timestamp(mills: i64) -> DateTime<Local> {
    let time: DateTime<Local> = Local.timestamp_millis(mills);

    time
}
