use crate::{Context, Vars, scheduler::Task};
use regex::Regex;
use serde_json::Value as JsonValue;
use std::sync::Arc;

pub fn fill_params(params: &JsonValue, ctx: &Context) -> JsonValue {
    match params {
        JsonValue::String(value) => {
            let exprs = get_exprs(value);
            if !exprs.is_empty() {
                let mut value = value.clone();
                for (range, expr) in &exprs {
                    let result = Context::scope(ctx.clone(), move || {
                        ctx.runtime.env().eval::<JsonValue>(expr)
                    })
                    .unwrap_or_else(|err| {
                        eprintln!("fill_params: expr:{value}, err={err}");
                        JsonValue::Null
                    });
                    // just return json for only one express
                    if range.start == 0 && range.end == value.len() {
                        return result;
                    }

                    match result {
                        JsonValue::Bool(v) => {
                            value = value.replace(expr, &v.to_string());
                        }
                        JsonValue::Number(v) => {
                            value = value.replace(expr, &v.to_string());
                        }
                        JsonValue::String(v) => {
                            value = value.replace(expr, &v);
                        }
                        v => {
                            value = value.replace(expr, &v.to_string());
                        }
                    }
                }
                // return string json for multiple expressions
                return JsonValue::String(value);
            }

            // return params itself for no expression
            params.clone()
        }
        JsonValue::Array(values) => {
            let mut arr = Vec::new();
            for value in values {
                arr.push(fill_params(value, ctx));
            }
            JsonValue::Array(arr)
        }
        JsonValue::Object(map) => {
            let mut obj = serde_json::Map::new();
            for (k, value) in map {
                obj.insert(k.clone(), fill_params(value, ctx));
            }
            JsonValue::Object(obj)
        }
        v => v.clone(),
    }
}

/// fill the vars
/// 1. if the inputs is an expression, just calculate it
///    or insert the input itself
pub fn fill_inputs(inputs: &Vars, ctx: &Context) -> Vars {
    let mut ret = Vars::new();
    for (k, ref v) in inputs {
        if let JsonValue::String(value) = v {
            if let Some(expr) = get_expr(value) {
                let result = Context::scope(ctx.clone(), move || {
                    ctx.runtime.env().eval::<JsonValue>(&expr)
                });
                let new_value = result.unwrap_or_else(|err| {
                    eprintln!("fill_inputs: expr:{value}, err={err}");
                    JsonValue::Null
                });

                // satisfies the rule 1
                ret.insert(k.to_string(), new_value);
                continue;
            }
        } else if let JsonValue::Object(obj) = v {
            ret.insert(k.to_string(), fill_inputs(&obj.clone().into(), ctx).into());
            continue;
        }
        ret.insert(k.to_string(), v.clone());
    }

    ret
}

/// fill the outputs
/// 1. if the outputs is an expression, just calculate it
/// 2. if the env and the outputs both has the same key, using the local outputs
pub fn fill_outputs(outputs: &Vars, ctx: &Context) -> Vars {
    // println!("fill_outputs: outputs={outputs}");
    let mut ret = Vars::new();
    for (ref k, ref v) in outputs {
        if let JsonValue::String(string) = v
            && let Some(expr) = get_expr(string)
        {
            let result = Context::scope(ctx.clone(), move || {
                ctx.runtime.env().eval::<JsonValue>(&expr)
            });
            let new_value = result.unwrap_or_else(|err| {
                eprintln!("fill_outputs: expr:{string}, err={err}");
                JsonValue::Null
            });

            // satisfies the rule 1
            ret.insert(k.to_string(), new_value);
            continue;
        }

        // rule 2
        if v.is_null() {
            // the env value
            match ctx.task().find(k) {
                Some(v) => ret.insert(k.to_string(), v),
                None => ret.insert(k.to_string(), v.clone()),
            };
        } else {
            // insert the orign value
            ret.insert(k.to_string(), v.clone());
        }
    }

    ret
}

pub fn fill_proc_vars(task: &Arc<Task>, values: &Vars, ctx: &Context) -> Vars {
    let mut ret = Vars::new();
    for (ref k, ref v) in values {
        if let JsonValue::String(string) = v
            && let Some(expr) = get_expr(string)
        {
            let result = Context::scope(ctx.clone(), || ctx.runtime.env().eval::<JsonValue>(&expr));
            let new_value = result.unwrap_or(JsonValue::Null);

            // satisfies the rule 1
            ret.insert(k.to_string(), new_value);

            continue;
        }

        // rule 2
        match task.find::<JsonValue>(k) {
            Some(v) => ret.insert(k.to_string(), v.clone()),
            None => ret.insert(k.to_string(), v.clone()),
        };
    }
    ret
}

pub fn get_expr(text: &str) -> Option<String> {
    let re = Regex::new(r"^\{\{(.+)\}\}$").unwrap();
    let caps = re.captures(text);

    if let Some(caps) = caps {
        let value = caps.get(1).map_or("", |m| m.as_str());
        return Some(value.trim().to_string());
    }

    None
}

pub fn get_exprs(text: &str) -> Vec<(core::ops::Range<usize>, String)> {
    let re = Regex::new(r"\{\{(.*)\}\}").unwrap();
    re.find_iter(text)
        .map(|cap| (cap.range(), cap.as_str().to_string()))
        .collect()
}
