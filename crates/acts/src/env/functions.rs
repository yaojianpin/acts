use crate::{Context, Result, TimeoutLimit, Vars, utils::consts};
use cel_interpreter::{
    Context as CelContext, ExecutionError, Value, extractors::Arguments, objects::ResolveResult,
};
use serde_json::Value as JsonValue;
use std::sync::LazyLock;

/// The identifier prefix `$name` is rewritten to before compilation. CEL
/// identifiers cannot contain `$`, so the engine's `$`-prefixed built-ins
/// (`$env`, `$get`, `$profile`, …) map to this prefix while user data (task
/// vars, user vars, step ids) keep their bare names.
pub(crate) const DOLLAR_PREFIX: &str = "__acts_";

/// Rewrite a source expression so `$name` becomes `__acts_name`, leaving
/// string literals and backtick-escaped identifiers untouched.
pub(crate) fn rewrite(expr: &str) -> String {
    let chars: Vec<char> = expr.chars().collect();
    let mut out = String::with_capacity(expr.len() + 8);
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '"' | '\'' => {
                let quote = chars[i];
                out.push(quote);
                i += 1;
                while i < chars.len() {
                    let c = chars[i];
                    out.push(c);
                    i += 1;
                    if c == '\\' && i < chars.len() {
                        out.push(chars[i]);
                        i += 1;
                    } else if c == quote {
                        break;
                    }
                }
            }
            '`' => {
                out.push('`');
                i += 1;
                while i < chars.len() && chars[i] != '`' {
                    out.push(chars[i]);
                    i += 1;
                }
                if i < chars.len() {
                    out.push('`');
                    i += 1;
                }
            }
            '$' => {
                if i + 1 < chars.len() && is_ident_start(chars[i + 1]) {
                    out.push_str(DOLLAR_PREFIX);
                    i += 1;
                    while i < chars.len() && is_ident_continue(chars[i]) {
                        out.push(chars[i]);
                        i += 1;
                    }
                } else {
                    out.push('$');
                    i += 1;
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident_continue(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The shared function registry — the CEL built-ins plus the engine's
/// `__acts_*` functions — initialized once and reused by every evaluation.
/// Functions are context-independent; only the variables are re-injected per
/// evaluation (see [`inject_vars`]).
static ROOT: LazyLock<CelContext<'static>> = LazyLock::new(|| {
    let mut ctx = CelContext::default();
    register(&mut ctx);
    ctx
});

/// The static root context every evaluation derives a child scope from.
pub(crate) fn root() -> &'static CelContext<'static> {
    &ROOT
}

/// Register the engine's built-in CEL functions under their `__acts_` names.
pub(crate) fn register(ctx: &mut cel_interpreter::Context) {
    ctx.add_function(
        "__acts_get",
        |name: std::sync::Arc<String>| -> ResolveResult {
            let value = Context::with(|ctx| ctx.task().find::<JsonValue>(&name));
            Ok(json_to_cel(value.unwrap_or(JsonValue::Null)))
        },
    );

    ctx.add_function(
        "__acts_set",
        |name: std::sync::Arc<String>, value: Value| -> ResolveResult {
            let value = cel_to_json(&value)?;
            let vars = Vars::new().with(name.as_str(), value);
            Context::with(|ctx| ctx.task().update_data(&vars));
            Ok(Value::Null)
        },
    );

    ctx.add_function(
        "__acts_set_process_var",
        |name: std::sync::Arc<String>, value: Value| -> ResolveResult {
            let value = cel_to_json(&value)?;
            let vars = Vars::new().with(name.as_str(), value);
            Context::with(|ctx| {
                ctx.proc.set_data(&vars);
                if let Some(root) = ctx.proc.root() {
                    let _ = ctx.runtime.try_upsert_task(&root);
                }
            });
            Ok(Value::Null)
        },
    );

    ctx.add_function("__acts_inputs", || -> ResolveResult {
        let inputs = Context::with(|ctx| ctx.task().inputs());
        Ok(json_to_cel(inputs.into()))
    });

    ctx.add_function("__acts_data", || -> ResolveResult {
        let data = Context::with(|ctx| ctx.task().data());
        Ok(json_to_cel(data.into()))
    });

    ctx.add_function("__acts_ecode", || -> ResolveResult {
        let ecode = Context::with(|ctx| ctx.task().data().get::<String>(consts::ACT_ERR_CODE));
        Ok(json_to_cel(
            ecode.map_or(JsonValue::Null, JsonValue::String),
        ))
    });

    ctx.add_function("__acts_cost", || -> i64 {
        Context::with(|ctx| current_cost(&ctx.task()))
    });

    ctx.add_function(
        "__acts_cost_in",
        |Arguments(args): Arguments| -> ResolveResult {
            let mut iter = args.iter();
            let Some(Value::String(min)) = iter.next() else {
                return Err(ExecutionError::function_error(
                    "cost_in",
                    "min must be a string",
                ));
            };
            let max = match iter.next() {
                Some(Value::String(max)) => Some(max.as_str().to_string()),
                Some(_) => {
                    return Err(ExecutionError::function_error(
                        "cost_in",
                        "max must be a string",
                    ));
                }
                None => None,
            };
            if iter.next().is_some() {
                return Err(ExecutionError::invalid_argument_count(1, args.len()));
            }
            let cost = Context::with(|ctx| current_cost(&ctx.task()));
            Ok(Value::Bool(cost_in(cost, min.as_str(), max.as_deref())))
        },
    );

    ctx.add_function(
        "__acts_step_value",
        |nid: std::sync::Arc<String>, name: std::sync::Arc<String>| -> ResolveResult {
            let value = Context::with(|ctx| step_data(ctx, &nid).and_then(|d| d.get(&name)));
            Ok(json_to_cel(value.unwrap_or(JsonValue::Null)))
        },
    );

    ctx.add_function(
        "__acts_step_data",
        |nid: std::sync::Arc<String>| -> ResolveResult {
            let data = Context::with(|ctx| step_data(ctx, &nid));
            Ok(json_to_cel(data.map_or(JsonValue::Null, Vars::into)))
        },
    );

    ctx.add_function(
        "__acts_step_inputs",
        |nid: std::sync::Arc<String>| -> ResolveResult {
            let inputs = Context::with(|ctx| {
                ctx.proc
                    .find_tasks(|task| task.node().id() == nid.as_str())
                    .last()
                    .map(|task| task.inputs())
            });
            Ok(json_to_cel(inputs.map_or(JsonValue::Null, Vars::into)))
        },
    );

    ctx.add_function(
        "__acts_set_step_value",
        |nid: std::sync::Arc<String>,
         name: std::sync::Arc<String>,
         value: Value|
         -> ResolveResult {
            let value = cel_to_json(&value)?;
            let vars = Vars::new().with(name.as_str(), value);
            let result: crate::Result<()> = Context::with(|ctx| {
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
            result.map_err(|err| ExecutionError::function_error("set_step_value", err))?;
            Ok(Value::Null)
        },
    );

    ctx.add_function("__acts_steps", || -> ResolveResult {
        let steps = Context::with(|ctx| {
            ctx.proc
                .tasks()
                .iter()
                .filter(|task| task.is_kind(crate::NodeKind::Step))
                .map(|task| Value::from(task.node().id()))
                .collect::<Vec<_>>()
        });
        Ok(Value::List(steps.into()))
    });
}

/// Inject the per-evaluation variables: task vars and user vars as bare
/// identifiers, step data as bare step-id maps, `$env`/`os` and sealed `$name`
/// under their `__acts_` names.
pub(crate) fn inject_vars(
    env: &super::Environment,
    ctx: &mut cel_interpreter::Context,
) -> Result<()> {
    // 1. task vars (merged lineage) as bare identifiers
    let task_vars = Context::try_with_current(|ctx| ctx.task().vars()).unwrap_or_default();
    for (key, value) in task_vars.iter() {
        ctx.add_variable_from_value(key.clone(), json_to_cel(value.clone()));
    }

    // 2. user vars (`secrets` and any embedder-registered var) as bare ids
    for (name, data) in env.user_vars() {
        ctx.add_variable_from_value(name, json_to_cel(data.into()));
    }

    // 3. `$env` — system env overlaid with the process env, minus the
    // engine's private keys; `WORK_DIR` is answered from the process.
    ctx.add_variable_from_value(format!("{DOLLAR_PREFIX}env"), json_to_cel(env_map().into()));

    // 4. `$os`
    ctx.add_variable_from_value(format!("{DOLLAR_PREFIX}os"), std::env::consts::OS);

    // 5. step data as bare step-id maps (reads `step1.a` etc.)
    if let Ok(cx) = Context::try_with_current(|c| c.clone()) {
        let steps: Vec<String> = cx
            .proc
            .tasks()
            .iter()
            .filter(|task| task.is_kind(crate::NodeKind::Step))
            .map(|task| task.node().id().to_string())
            .collect();
        for nid in steps {
            let data = step_data(&cx, &nid).unwrap_or_default();
            ctx.add_variable_from_value(nid, json_to_cel(data.into()));
        }
    }

    // 6. sealed `$name` — walk the task's sealed chain once.
    if let Ok(cx) = Context::try_with_current(|c| c.clone()) {
        let task = cx.task();
        let mut names: Vec<String> = Vec::new();
        let mut cursor = Some(task.clone());
        while let Some(t) = cursor {
            for name in t.sealed_keys() {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
            cursor = t.parent();
        }
        for name in names {
            if let Some(data) = task.sealed(&name) {
                ctx.add_variable_from_value(
                    format!("{DOLLAR_PREFIX}{name}"),
                    json_to_cel(data.into()),
                );
            }
        }
    }

    Ok(())
}

fn step_data(ctx: &Context, nid: &str) -> Option<Vars> {
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

fn json_to_cel(value: JsonValue) -> Value {
    match value {
        JsonValue::Null => Value::Null,
        JsonValue::Bool(b) => Value::Bool(b),
        // serde_json stores every non-negative integer as a u64, which CEL
        // reads as `uint`; a `uint` does not mix with the `int` literals of
        // arithmetic (`index + 1`). Narrow a u64 that fits an i64 to `int` so
        // non-negative workflow numbers behave like the int literals they are
        // written against; a genuinely wider value stays `uint`.
        JsonValue::Number(n) => {
            if let Some(u) = n.as_u64() {
                if let Ok(i) = i64::try_from(u) {
                    Value::Int(i)
                } else {
                    Value::UInt(u)
                }
            } else if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else {
                Value::Float(n.as_f64().unwrap_or_default())
            }
        }
        JsonValue::String(s) => Value::String(s.into()),
        JsonValue::Array(arr) => Value::List(std::sync::Arc::new(
            arr.into_iter().map(json_to_cel).collect(),
        )),
        JsonValue::Object(map) => {
            let map: std::collections::HashMap<String, Value> =
                map.into_iter().map(|(k, v)| (k, json_to_cel(v))).collect();
            Value::Map(map.into())
        }
    }
}

fn cel_to_json(value: &Value) -> std::result::Result<JsonValue, ExecutionError> {
    value
        .json()
        .map_err(|err| ExecutionError::function_error("value", err))
}
