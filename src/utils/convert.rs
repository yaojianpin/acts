use crate::{
    event::MessageState,
    sch::{Task, TaskState},
    Context, Vars,
};
use regex::Regex;
use serde_json::Value as JsonValue;
use std::sync::Arc;

/// fill the vars
/// 1. if the inputs is an expression, just calculate it
/// or insert the input itself
pub fn fill_inputs<'a>(inputs: &'a Vars, ctx: &Context) -> Vars {
    let mut ret = Vars::new();
    for (k, v) in inputs {
        if let JsonValue::String(string) = v {
            if let Some(expr) = get_expr(string) {
                let result = Context::scope(ctx.clone(), move || ctx.env.eval::<JsonValue>(&expr));
                let new_value = match result {
                    Ok(v) => v,
                    Err(err) => {
                        eprintln!("fill_inputs: {err}");
                        JsonValue::Null
                    }
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
/// 2. if the env and the outputs both has the same key, using the local outputs
pub fn fill_outputs(outputs: &Vars, ctx: &Context) -> Vars {
    // println!("fill_outputs: outputs={outputs}");
    let mut ret = Vars::new();
    for (k, v) in outputs {
        if let JsonValue::String(string) = v {
            if let Some(expr) = get_expr(string) {
                let result = Context::scope(ctx.clone(), move || ctx.env.eval::<JsonValue>(&expr));
                let new_value = match result {
                    Ok(v) => v,
                    Err(_err) => JsonValue::Null,
                };

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

pub fn fill_proc_vars<'a>(task: &Arc<Task>, values: &'a Vars, ctx: &Context) -> Vars {
    let mut ret = Vars::new();
    for (k, v) in values {
        if let JsonValue::String(string) = v {
            if let Some(expr) = get_expr(string) {
                let result = Context::scope(ctx.clone(), || ctx.env.eval::<JsonValue>(&expr));
                let new_value = match result {
                    Ok(v) => v,
                    Err(_err) => JsonValue::Null,
                };

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
    let caps = re.captures(&text);

    if let Some(caps) = caps {
        let value = caps.get(1).map_or("", |m| m.as_str());
        return Some(value.trim().to_string());
    }

    None
}

pub fn message_state_to_str(state: MessageState) -> String {
    match state {
        MessageState::None => "none".to_string(),
        MessageState::Aborted => "aborted".to_string(),
        MessageState::Backed => "backed".to_string(),
        MessageState::Cancelled => "cancelled".to_string(),
        MessageState::Completed => "completed".to_string(),
        MessageState::Created => "created".to_string(),
        MessageState::Skipped => "skipped".to_string(),
        MessageState::Submitted => "submitted".to_string(),
        MessageState::Error => "error".to_string(),
        MessageState::Removed => "removed".to_string(),
    }
}

pub fn str_to_message_state(s: &str) -> MessageState {
    match s {
        "aborted" => MessageState::Aborted,
        "backed" => MessageState::Backed,
        "cancelled" => MessageState::Cancelled,
        "completed" => MessageState::Completed,
        "created" => MessageState::Created,
        "skipped" => MessageState::Skipped,
        "submitted" => MessageState::Submitted,
        "error" => MessageState::Error,
        "removed" => MessageState::Removed,
        "none" | _ => MessageState::None,
    }
}

pub fn state_to_str(state: TaskState) -> String {
    match state {
        TaskState::Pending => "pending".to_string(),
        TaskState::Running => "running".to_string(),
        TaskState::Interrupt => "interrupt".to_string(),
        TaskState::Completed => "completed".to_string(),
        TaskState::Submitted => "submitted".to_string(),
        TaskState::Backed => "backed".to_string(),
        TaskState::Cancelled => "cancelled".to_string(),
        TaskState::Error => "fail".to_string(),
        TaskState::Skipped => "skip".to_string(),
        TaskState::Aborted => "abort".to_string(),
        TaskState::Removed => "removed".to_string(),
        TaskState::None => "none".to_string(),
    }
}

pub fn str_to_state(str: &str) -> TaskState {
    match str {
        "none" => TaskState::None,
        "pending" => TaskState::Pending,
        "running" => TaskState::Running,
        "ok" => TaskState::Completed,
        "skip" => TaskState::Skipped,
        "abort" => TaskState::Aborted,
        "interrupt" => TaskState::Interrupt,
        "fail" => TaskState::Error,
        _ => TaskState::None,
    }
}
