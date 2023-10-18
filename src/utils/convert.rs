use std::collections::HashMap;

use crate::{
    env::{Enviroment, Room},
    event::ActionState,
    sch::TaskState,
    ActError, ActValue, Result, Vars,
};
use regex::Regex;
use rhai::{Dynamic, Map};
use serde_json::Map as JsonMap;

pub fn value_to_dymainc(v: &ActValue) -> Dynamic {
    match v {
        ActValue::Null => Dynamic::UNIT,
        ActValue::Bool(b) => Dynamic::from(b.clone()),
        ActValue::String(s) => Dynamic::from(s.clone()),
        ActValue::Number(n) if n.is_i64() => Dynamic::from(n.as_i64().unwrap()),
        ActValue::Number(n) if n.is_f64() => Dynamic::from(n.as_f64().unwrap()),
        ActValue::Array(s) => Dynamic::from(array_to_dynamic(s)),
        ActValue::Object(m) => Dynamic::from(map_to_dynamic(m)),
        _ => Dynamic::default(),
    }
}

pub fn dynamic_to_value(value: &Dynamic) -> ActValue {
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
    } else if value.is::<rhai::Array>() {
        let arr = value.clone().into_array().unwrap();
        let arr_values: Vec<_> = arr.iter().map(|v| dynamic_to_value(v)).collect();

        return ActValue::Array(arr_values);
    }

    ActValue::Null
}

pub fn array_to_dynamic<'a>(values: &'a Vec<ActValue>) -> Vec<Dynamic> {
    let mut ret = Vec::new();

    for v in values {
        ret.push(value_to_dymainc(v));
    }

    ret
}

pub fn map_to_dynamic<'a>(map: &JsonMap<String, ActValue>) -> Map {
    let mut ret: Map = Map::new();
    for (k, v) in map {
        let key = k.to_string();
        ret.insert(key.into(), value_to_dymainc(v));
    }

    ret
}

/// fill the vars
/// 1. if the inputs is an expression, just calculate it
/// or insert the input itself
pub fn fill_inputs<'a>(env: &Room, inputs: &'a Vars) -> Vars {
    let mut ret = Vars::new();
    for (k, v) in inputs {
        if let ActValue::String(string) = v {
            if let Some(expr) = get_expr(string) {
                let result = env.eval::<Dynamic>(&expr);
                let new_value = match result {
                    Ok(v) => dynamic_to_value(&v),
                    Err(_err) => ActValue::Null,
                };

                // satisfies the rule 1
                ret.insert(k.to_string(), new_value);
                continue;
            }
        }
        ret.insert(k.to_string(), v.clone());
    }

    ret
}

/// fill the outputs
/// 1. if the outputs is an expression, just calculate it
/// 2. if the env and the outpus both has the same key, using the local outputs
pub fn fill_outputs(env: &Room, outputs: &Vars) -> Vars {
    let mut ret = Vars::new();

    for (k, v) in outputs {
        if let ActValue::String(string) = v {
            if let Some(expr) = get_expr(string) {
                let result = env.eval::<Dynamic>(&expr);
                let new_value = match result {
                    Ok(v) => dynamic_to_value(&v),
                    Err(_err) => ActValue::Null,
                };

                // satisfies the rule 1
                ret.insert(k.to_string(), new_value);
                continue;
            }
        }

        // rule 2
        match env.get(k) {
            Some(v) => ret.insert(k.to_string(), v.clone()),
            None => ret.insert(k.to_string(), v.clone()),
        };
    }

    ret
}

pub fn fill_proc_vars<'a>(env: &Enviroment, values: &'a Vars) -> Vars {
    let mut ret = Vars::new();

    for (k, v) in values {
        if let ActValue::String(string) = v {
            if let Some(expr) = get_expr(string) {
                let result = env.eval::<Dynamic>(&expr);
                let new_value = match result {
                    Ok(v) => dynamic_to_value(&v),
                    Err(_err) => ActValue::Null,
                };

                // satisfies the rule 1
                ret.insert(k.to_string(), new_value);
                continue;
            }
        }

        // rule 2
        match env.get(k) {
            Some(v) => ret.insert(k.to_string(), v.clone()),
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

pub fn action_state_to_str(state: ActionState) -> String {
    match state {
        ActionState::None => "none".to_string(),
        ActionState::Aborted => "aborted".to_string(),
        ActionState::Backed => "backed".to_string(),
        ActionState::Cancelled => "cancelled".to_string(),
        ActionState::Completed => "completed".to_string(),
        ActionState::Created => "created".to_string(),
        ActionState::Skipped => "skipped".to_string(),
        ActionState::Submitted => "submitted".to_string(),
        ActionState::Error => "error".to_string(),
    }
}

pub fn str_to_action_state(s: &str) -> ActionState {
    match s {
        "aborted" => ActionState::Aborted,
        "backed" => ActionState::Backed,
        "cancelled" => ActionState::Cancelled,
        "completed" => ActionState::Completed,
        "created" => ActionState::Created,
        "skipped" => ActionState::Skipped,
        "submitted" => ActionState::Submitted,
        "err" => ActionState::Error,
        "none" | _ => ActionState::None,
    }
}

pub fn state_to_str(state: TaskState) -> String {
    match state {
        TaskState::Pending => "pending".to_string(),
        TaskState::Running => "running".to_string(),
        TaskState::Success => "success".to_string(),
        TaskState::Fail(s) => format!("fail({})", s),
        TaskState::Skip => "skip".to_string(),
        TaskState::Abort => "abort".to_string(),
        TaskState::None => "none".to_string(),
    }
}

pub fn str_to_state(str: &str) -> TaskState {
    let re = regex::Regex::new(r"^(.*)\((.*)\)$").unwrap();
    match str {
        "none" => TaskState::None,
        "pending" => TaskState::Pending,
        "running" => TaskState::Running,
        "success" => TaskState::Success,
        "skip" => TaskState::Skip,
        "abort" => TaskState::Abort,
        _ => {
            let caps = re.captures(str);
            if let Some(caps) = caps {
                let name = caps.get(1).map_or("", |m| m.as_str());
                let err = caps.get(2).map_or("", |m| m.as_str());

                if name == "fail" {
                    return TaskState::Fail(err.to_string());
                }
            }

            TaskState::None
        }
    }
}

pub fn value_to_hash_map(value: &ActValue) -> Result<HashMap<String, Vec<String>>> {
    let arr = value.as_object().ok_or(ActError::Convert(format!(
        "cannot convert json '{}' to hash_map, it is not object",
        value
    )))?;

    let mut ret = HashMap::new();
    for (k, v) in arr {
        let items = v
            .as_array()
            .ok_or(ActError::Convert(format!(
                "cannot convert json to vec, it is not array type in key '{}'",
                k
            )))?
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        ret.insert(k.to_string(), items);
    }
    Ok(ret)
}
