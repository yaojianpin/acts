use crate::{
    env::{Enviroment, VirtualMachine},
    ActValue, Vars,
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
/// 2. if the env and the outpus both has the same key, using the env to replace the it in outputs
pub fn fill_vars<'a>(env: &VirtualMachine, values: &'a Vars) -> Vars {
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

// pub fn fmt_timestamp(
//     start_time_mills: i64,
//     end_time_mills: i64,
//     fmt: &str,
// ) -> (String, String, i64) {
//     let start_time = parse_timestamp(start_time_mills);
//     let end_time = parse_timestamp(end_time_mills);

//     let elapsed = end_time_mills - start_time_mills;

//     (
//         start_time.format(fmt).to_string(),
//         end_time.format(fmt).to_string(),
//         elapsed,
//     )
// }

// fn parse_timestamp(mills: i64) -> DateTime<Local> {
//     let time: DateTime<Local> = Local.timestamp_millis(mills);

//     time
// }
