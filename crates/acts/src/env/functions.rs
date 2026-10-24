//! The engine's expression built-ins and the variables an expression reads.
//!
//! Nothing is injected that the expression does not name. The compiled
//! expression reports the names it references ([`acts_expr::Expr`]), and each
//! one is resolved here on demand — so the whole `$env` snapshot, every step's
//! data and the sealed chain cost nothing to an expression that never reads
//! them, where the previous evaluator built the complete scope for every
//! evaluation whether or not the expression looked at any of it.

use crate::{Context, Result, TimeoutLimit, Vars, utils::consts};
use acts_expr::{Context as ExprContext, Error as ExprError, Value};
use serde_json::Value as JsonValue;
use std::sync::Arc;

/// Bind the referenced names the current task can answer.
///
/// A name that resolves to nothing is left unbound, so reading it fails as an
/// unknown variable — the undeclared reference the engine reported before.
pub(crate) fn inject(
    env: &super::Environment,
    ctx: &mut ExprContext,
    references: &[&str],
) -> Result<()> {
    for name in references {
        if let Some(value) = resolve(env, name) {
            ctx.set(*name, value);
        }
    }

    Ok(())
}

/// One referenced name.
///
/// The precedence is the one the engine has always injected them in, where the
/// last written won: a sealed `$name` over a built-in of the same name, and
/// step data over the user vars over the task's own vars.
fn resolve(env: &super::Environment, name: &str) -> Option<Value> {
    if let Some(sealed) = name.strip_prefix('$') {
        if let Some(value) = sealed_data(sealed) {
            return Some(value);
        }
        if let Some(value) = builtin(name) {
            return Some(value);
        }
        // Not a built-in: a `$`-prefixed name is still a task var when a
        // pushed act injected one (`$index`, `$value`).
    }

    if let Some(data) = step_data(name) {
        return Some(from_json(JsonValue::from(data)));
    }
    if let Some(data) = env.user_var(name) {
        return Some(from_json(JsonValue::from(data)));
    }

    task_var(name).map(from_json)
}

/// A sealed `$name` of this task or of an ancestor (child overrides parent).
fn sealed_data(name: &str) -> Option<Value> {
    Context::try_with_current(|ctx| ctx.task().sealed(&ctx.proc, name))
        .ok()
        .flatten()
        .map(|data| from_json(JsonValue::from(data)))
}

/// A task var of this task or of an ancestor, merged the way `find` reads.
fn task_var(name: &str) -> Option<JsonValue> {
    Context::try_with_current(|ctx| ctx.task().find::<JsonValue>(&ctx.proc, name))
        .ok()
        .flatten()
}

/// The data of the newest generation of the task whose node id is `nid`, which
/// is how a step's own data is read as a bare step id (`step1.a`). A step id
/// is not part of the local data map: it is looked up in the shared aggregate
/// only when the expression references it.
fn step_data(nid: &str) -> Option<Vars> {
    Context::try_with_current(|ctx| ctx.task().step_data(&ctx.proc, nid))
        .ok()
        .flatten()
}

/// `$env`, `$os` and the functions the engine exposes to an expression.
fn builtin(name: &str) -> Option<Value> {
    Some(match name {
        // `$env`: the system environment overlaid with the process's, minus
        // the engine's private keys; `WORK_DIR` is answered from the process.
        "$env" => from_json(JsonValue::from(env_map())),
        "$os" => Value::from(std::env::consts::OS),

        "$get" => Value::function(|args| {
            arity("$get", args, 1, 1)?;
            let name = str_arg("$get", args, 0)?;
            let value = Context::with(|ctx| ctx.task().find::<JsonValue>(&ctx.proc, &name));

            Ok(from_json(value.unwrap_or(JsonValue::Null)))
        }),
        "$inputs" => Value::function(|args| {
            arity("$inputs", args, 0, 0)?;

            Ok(from_json(JsonValue::from(Context::with(|ctx| {
                ctx.task().inputs(ctx)
            }))))
        }),
        "$data" => Value::function(|args| {
            arity("$data", args, 0, 0)?;

            Ok(from_json(JsonValue::from(Context::with(|ctx| {
                ctx.task().data()
            }))))
        }),
        "$ecode" => Value::function(|args| {
            arity("$ecode", args, 0, 0)?;
            let ecode =
                Context::with(|ctx| ctx.task().find::<String>(&ctx.proc, consts::ACT_ERR_CODE));

            Ok(match ecode {
                Some(ecode) => Value::from(ecode),
                None => Value::Null,
            })
        }),
        "$cost" => Value::function(|args| {
            arity("$cost", args, 0, 0)?;

            Ok(Value::from(Context::with(|ctx| {
                current_cost(&ctx.task(), &ctx.proc)
            })))
        }),
        "$cost_in" => Value::function(|args| {
            arity("$cost_in", args, 1, 2)?;
            let min = str_arg("$cost_in", args, 0)?;
            let max = match args.get(1) {
                Some(_) => Some(str_arg("$cost_in", args, 1)?),
                None => None,
            };
            let cost = Context::with(|ctx| current_cost(&ctx.task(), &ctx.proc));

            Ok(Value::Bool(cost_in(cost, &min, max.as_deref())))
        }),
        "$step_value" => Value::function(|args| {
            arity("$step_value", args, 2, 2)?;
            let nid = str_arg("$step_value", args, 0)?;
            let name = str_arg("$step_value", args, 1)?;
            let value = step_data(&nid).and_then(|data| data.get::<JsonValue>(&name));

            Ok(from_json(value.unwrap_or(JsonValue::Null)))
        }),
        "$step_data" => Value::function(|args| {
            arity("$step_data", args, 1, 1)?;
            let nid = str_arg("$step_data", args, 0)?;
            let data = step_data(&nid);

            Ok(match data {
                Some(data) => from_json(JsonValue::from(data)),
                None => Value::Null,
            })
        }),
        "$step_inputs" => Value::function(|args| {
            arity("$step_inputs", args, 1, 1)?;
            let nid = str_arg("$step_inputs", args, 0)?;
            let inputs = Context::try_with_current(|ctx| ctx.task().step_inputs(&ctx.proc, &nid))
                .ok()
                .flatten();

            Ok(match inputs {
                Some(inputs) => from_json(JsonValue::from(inputs)),
                None => Value::Null,
            })
        }),
        "$steps" => Value::function(|args| {
            arity("$steps", args, 0, 0)?;
            let steps =
                Context::try_with_current(|ctx| ctx.task().steps(&ctx.proc)).unwrap_or_default();

            Ok(Value::List(
                steps
                    .into_iter()
                    .map(Value::from)
                    .collect::<Vec<_>>()
                    .into(),
            ))
        }),

        _ => return None,
    })
}

/// Exactly `min..=max` arguments, or the error that names the built-in.
fn arity(name: &str, args: &[Value], min: usize, max: usize) -> std::result::Result<(), ExprError> {
    if args.len() >= min && args.len() <= max {
        return Ok(());
    }

    let expected = if min == max {
        format!("{min}")
    } else {
        format!("{min} to {max}")
    };

    Err(ExprError::Function {
        name: name.to_string(),
        message: format!("takes {expected} argument(s), got {}", args.len()),
    })
}

/// The `index`-th argument as a string.
fn str_arg(name: &str, args: &[Value], index: usize) -> std::result::Result<String, ExprError> {
    match args.get(index) {
        Some(Value::Str(value)) => Ok(value.to_string()),
        Some(other) => Err(ExprError::Function {
            name: name.to_string(),
            message: format!(
                "argument {} must be a string, got {}",
                index + 1,
                other.type_name()
            ),
        }),
        None => Err(ExprError::Function {
            name: name.to_string(),
            message: format!("argument {} is missing", index + 1),
        }),
    }
}

/// A JSON value in the evaluator's own value type.
fn from_json(value: JsonValue) -> Value {
    Value::from(value)
}

fn env_map() -> Vars {
    let mut map = Vars::new();

    // system env first: the process env overlays it below.
    for (k, v) in std::env::vars() {
        if !consts::is_private_key(&k) {
            map.set(&k, JsonValue::String(v));
        }
    }

    if let Ok(proc_env) = Context::try_with_current(|ctx| ctx.proc.env()) {
        for (k, v) in proc_env.iter() {
            if !consts::is_private_key(k) {
                map.set(k, v.clone());
            }
        }
    }

    // The directory a process runs in has a readable name of its own,
    // answered from the process rather than stored.
    if let Ok(Some(dir)) =
        Context::try_with_current(|ctx| ctx.get_env::<String>(consts::ENV_WORK_DIR))
    {
        map.set(consts::ENV_WORK_DIR, JsonValue::String(dir));
    }

    map
}

fn current_cost(task: &crate::scheduler::ScopeTask, scope: &Arc<crate::scheduler::Scope>) -> i64 {
    if let Some(cost) = task.find::<i64>(scope, consts::TASK_COST) {
        return cost;
    }
    task.cost()
}

fn cost_in(cost: i64, min: &str, max: Option<&str>) -> bool {
    let min_timeout = TimeoutLimit::parse(min).unwrap_or_default();
    let mut ret = cost >= min_timeout.as_secs() * 1000;
    if ret && let Some(max) = max {
        let max_timeout = TimeoutLimit::parse(max).unwrap_or_default();
        ret &= cost < max_timeout.as_secs() * 1000;
    }
    ret
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, args: &[Value]) -> std::result::Result<Value, ExprError> {
        match builtin(name).expect("the built-in exists") {
            Value::Function(function) => function(args),
            _ => panic!("{name} is not a function"),
        }
    }

    fn error(name: &str, args: &[Value]) -> String {
        call(name, args).unwrap_err().to_string()
    }

    /// `arity` names the accepted count for a fixed and for a ranged built-in,
    /// from both the too-few and the too-many side.
    #[test]
    fn builtin_arity_names_the_expected_bounds() {
        let message = error("$step_inputs", &[]);
        assert!(message.contains("takes 1 argument(s), got 0"), "{message}");
        let message = error("$step_inputs", &[Value::from("a"), Value::from("b")]);
        assert!(message.contains("takes 1 argument(s), got 2"), "{message}");

        let message = error("$cost_in", &[]);
        assert!(
            message.contains("takes 1 to 2 argument(s), got 0"),
            "{message}"
        );
        let message = error(
            "$cost_in",
            &[Value::from("1s"), Value::from("2s"), Value::from("3s")],
        );
        assert!(
            message.contains("takes 1 to 2 argument(s), got 3"),
            "{message}"
        );

        // The accepted counts run: a headless step id has no data, and there
        // is no step list.
        assert_eq!(
            call("$step_inputs", &[Value::from("s1")]).unwrap(),
            Value::Null
        );
        match call("$steps", &[]).unwrap() {
            Value::List(items) => assert!(items.is_empty()),
            _ => panic!("$steps is a list"),
        }
    }

    /// A non-string argument and a missing one each report their position.
    #[test]
    fn string_arguments_are_checked() {
        let message = error("$step_inputs", &[Value::Int(1)]);
        assert!(
            message.contains("argument 1 must be a string, got int"),
            "{message}"
        );
        let message = error("$cost_in", &[Value::from("1s"), Value::Int(2)]);
        assert!(
            message.contains("argument 2 must be a string, got int"),
            "{message}"
        );

        let message = str_arg("$x", &[], 0).unwrap_err().to_string();
        assert!(message.contains("argument 1 is missing"), "{message}");
        let message = str_arg("$x", &[], 1).unwrap_err().to_string();
        assert!(message.contains("argument 2 is missing"), "{message}");
    }

    /// The minimum alone, and the half-open maximum (`cost < max`) on top.
    #[test]
    fn cost_in_tests_the_minimum_and_the_optional_maximum() {
        assert!(!cost_in(0, "1s", None));
        assert!(cost_in(2_000, "1s", None));
        assert!(cost_in(2_000, "1s", Some("10s")));
        assert!(!cost_in(10_000, "1s", Some("10s")));
    }

    /// Without a scheduler context there is no process scope to read: `$env`
    /// still answers the process environment and never an engine-private key.
    #[test]
    fn env_map_reads_the_process_without_a_scheduler_context() {
        std::thread::spawn(|| {
            assert!(
                Context::try_with_current(|_| ()).is_err(),
                "a fresh thread has no context"
            );

            let map = env_map();
            let (key, value) = std::env::vars()
                .find(|(name, _)| !consts::is_private_key(name))
                .expect("the process has a non-private variable");
            assert_eq!(map.get::<String>(&key).as_deref(), Some(value.as_str()));

            match builtin("$env").expect("$env exists") {
                Value::Map(map) => assert!(!map.keys().any(|name| consts::is_private_key(name))),
                _ => panic!("$env is a map"),
            }
        })
        .join()
        .expect("the environment thread finishes");
    }
}
