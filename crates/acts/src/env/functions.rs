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
    Context::try_with_current(|ctx| ctx.task().sealed(name))
        .ok()
        .flatten()
        .map(|data| from_json(JsonValue::from(data)))
}

/// A task var of this task or of an ancestor, merged the way `find` reads.
fn task_var(name: &str) -> Option<JsonValue> {
    Context::try_with_current(|ctx| ctx.task().find::<JsonValue>(name))
        .ok()
        .flatten()
}

/// The data of the last task with node id `nid`, which is how a step's own
/// data is read as a bare step id (`step1.a`).
fn step_data(nid: &str) -> Option<Vars> {
    Context::try_with_current(|ctx| {
        ctx.proc
            .find_tasks(|task| task.node().id() == nid)
            .last()
            .map(|task| task.data())
    })
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
            let value = Context::with(|ctx| ctx.task().find::<JsonValue>(&name));

            Ok(from_json(value.unwrap_or(JsonValue::Null)))
        }),
        "$set" => Value::function(|args| {
            arity("$set", args, 2, 2)?;
            let name = str_arg("$set", args, 0)?;
            let value = json_arg("$set", args, 1)?;
            let vars = Vars::new().with(name.as_str(), value);
            Context::with(|ctx| ctx.task().update_data(&vars));

            Ok(Value::Null)
        }),
        "$set_process_var" => Value::function(|args| {
            arity("$set_process_var", args, 2, 2)?;
            let name = str_arg("$set_process_var", args, 0)?;
            let value = json_arg("$set_process_var", args, 1)?;
            let vars = Vars::new().with(name.as_str(), value);
            Context::with(|ctx| {
                ctx.proc.set_data(&vars);
                if let Some(root) = ctx.proc.root() {
                    let _ = ctx.runtime.try_upsert_task(&root);
                }
            });

            Ok(Value::Null)
        }),
        "$inputs" => Value::function(|args| {
            arity("$inputs", args, 0, 0)?;

            Ok(from_json(JsonValue::from(Context::with(|ctx| {
                ctx.task().inputs()
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
            let ecode = Context::with(|ctx| ctx.task().data().get::<String>(consts::ACT_ERR_CODE));

            Ok(match ecode {
                Some(ecode) => Value::from(ecode),
                None => Value::Null,
            })
        }),
        "$cost" => Value::function(|args| {
            arity("$cost", args, 0, 0)?;

            Ok(Value::from(Context::with(|ctx| current_cost(&ctx.task()))))
        }),
        "$cost_in" => Value::function(|args| {
            arity("$cost_in", args, 1, 2)?;
            let min = str_arg("$cost_in", args, 0)?;
            let max = match args.get(1) {
                Some(_) => Some(str_arg("$cost_in", args, 1)?),
                None => None,
            };
            let cost = Context::with(|ctx| current_cost(&ctx.task()));

            Ok(Value::Bool(cost_in(cost, &min, max.as_deref())))
        }),
        "$step_value" => Value::function(|args| {
            arity("$step_value", args, 2, 2)?;
            let nid = str_arg("$step_value", args, 0)?;
            let name = str_arg("$step_value", args, 1)?;
            let value = Context::with(|ctx| {
                step_data_of(ctx, &nid).and_then(|data| data.get::<JsonValue>(&name))
            });

            Ok(from_json(value.unwrap_or(JsonValue::Null)))
        }),
        "$step_data" => Value::function(|args| {
            arity("$step_data", args, 1, 1)?;
            let nid = str_arg("$step_data", args, 0)?;
            let data = Context::with(|ctx| step_data_of(ctx, &nid));

            Ok(match data {
                Some(data) => from_json(JsonValue::from(data)),
                None => Value::Null,
            })
        }),
        "$step_inputs" => Value::function(|args| {
            arity("$step_inputs", args, 1, 1)?;
            let nid = str_arg("$step_inputs", args, 0)?;
            let inputs = Context::with(|ctx| {
                ctx.proc
                    .find_tasks(|task| task.node().id() == nid.as_str())
                    .last()
                    .map(|task| task.inputs())
            });

            Ok(match inputs {
                Some(inputs) => from_json(JsonValue::from(inputs)),
                None => Value::Null,
            })
        }),
        "$set_step_value" => Value::function(|args| {
            arity("$set_step_value", args, 3, 3)?;
            let nid = str_arg("$set_step_value", args, 0)?;
            let name = str_arg("$set_step_value", args, 1)?;
            let value = json_arg("$set_step_value", args, 2)?;
            let vars = Vars::new().with(name.as_str(), value);
            let result: Result<()> = Context::with(|ctx| {
                let tasks = ctx.proc.find_tasks(|task| task.node().id() == nid.as_str());
                if let Some(task) = tasks.last() {
                    if task.state().is_completed() {
                        return Err(crate::ActError::Script(format!(
                            "Task with nid '{nid}' is already completed, cannot set value",
                        )));
                    }
                    task.update_data(&vars);
                    let _ = ctx.runtime.try_upsert_task(task);
                }
                Ok(())
            });
            result.map_err(|err| ExprError::Function {
                name: "$set_step_value".to_string(),
                message: err.to_string(),
            })?;

            Ok(Value::Null)
        }),
        "$steps" => Value::function(|args| {
            arity("$steps", args, 0, 0)?;
            let steps = Context::with(|ctx| {
                ctx.proc
                    .tasks()
                    .iter()
                    .filter(|task| task.is_kind(crate::NodeKind::Step))
                    .map(|task| Value::from(task.node().id()))
                    .collect::<Vec<_>>()
            });

            Ok(Value::List(steps.into()))
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

/// The `index`-th argument as JSON, which is the form the engine stores.
fn json_arg(name: &str, args: &[Value], index: usize) -> std::result::Result<JsonValue, ExprError> {
    match args.get(index) {
        Some(value) => value.to_json(),
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

fn step_data_of(ctx: &Context, nid: &str) -> Option<Vars> {
    ctx.proc
        .find_tasks(|task| task.node().id() == nid)
        .last()
        .map(|task| task.data())
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

fn current_cost(task: &crate::scheduler::Task) -> i64 {
    if let Some(cost) = task.data().get::<i64>(consts::TASK_COST) {
        return cost;
    }
    let mut parent = task.parent();
    while let Some(task) = parent {
        if let Some(cost) = task.data().get::<i64>(consts::TASK_COST) {
            return cost;
        }
        parent = task.parent();
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
