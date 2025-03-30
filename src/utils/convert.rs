use crate::{sch::Task, Context, Vars};
use regex::Regex;
use serde_json::Value as JsonValue;
use std::sync::Arc;

/// fill the vars
/// 1. if the inputs is an expression, just calculate it
///    or insert the input itself
pub fn fill_inputs(inputs: &Vars, ctx: &Context) -> Vars {
    let mut ret = Vars::new();
    for (k, ref v) in inputs {
        if let JsonValue::String(string) = v {
            if let Some(expr) = get_expr(string) {
                let result = Context::scope(ctx.clone(), move || {
                    ctx.runtime.env().eval::<JsonValue>(&expr)
                });
                let new_value = result.unwrap_or_else(|err| {
                    eprintln!("fill_inputs: expr:{string}, err={err}");
                    JsonValue::Null
                });

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
/// 2. if the env and the outputs both has the same key, using the local outputs
pub fn fill_outputs(outputs: &Vars, ctx: &Context) -> Vars {
    // println!("fill_outputs: outputs={outputs}");
    let mut ret = Vars::new();
    for (ref k, ref v) in outputs {
        if let JsonValue::String(string) = v {
            if let Some(expr) = get_expr(string) {
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
        if let JsonValue::String(string) = v {
            if let Some(expr) = get_expr(string) {
                let result =
                    Context::scope(ctx.clone(), || ctx.runtime.env().eval::<JsonValue>(&expr));
                let new_value = result.unwrap_or_else(|_err| JsonValue::Null);

                // satisfies the rule 1
                ret.insert(k.to_string(), new_value);

                continue;
            }
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
    let re = Regex::new(r"^\$\{(.+)\}$").unwrap();
    let caps = re.captures(text);

    if let Some(caps) = caps {
        let value = caps.get(1).map_or("", |m| m.as_str());
        return Some(value.trim().to_string());
    }

    None
}
