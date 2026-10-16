use super::*;

/// The declaring model of a snapshot, read from the durable root-scope
/// variable. `None` only for aggregates seeded before the nested import.
pub(super) fn model_of(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
) -> Result<Option<crate::model::Workflow>> {
    let Some(raw) = root_variable(snapshot, model::MODEL_VAR)? else {
        return Ok(None);
    };
    serde_json::from_str::<crate::model::Workflow>(&raw)
        .map(Some)
        .map_err(runtime)
}

/// The root execution of an aggregate: the `$` node's earliest generation —
/// the generation a re-entry never removes.
pub(super) fn root_task(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
) -> Option<crate::scheduler::core::model::task_graph::TaskId> {
    snapshot
        .tasks
        .keys()
        .filter(|id| id.node.as_str() == crate::utils::consts::TASK_ROOT_TID)
        .min_by_key(|id| id.generation)
        .cloned()
}

/// One root-scope variable (root = the `$` execution, earliest generation —
/// the generation a re-entry never removes).
pub(super) fn root_variable(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    name: &str,
) -> Result<Option<String>> {
    Ok(root_task(snapshot)
        .and_then(|root| snapshot.variables.get(&(root, name.to_string())).cloned()))
}

/// The root execution's state — what the slot rule bounds
/// (`crate::scheduler::core::plan::concurrency`). A snapshot without a root (no
/// seed produces one) reads as running, so the run asks for a slot instead of
/// being admitted for free.
pub(super) fn root_state(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
) -> crate::scheduler::core::model::task_graph::TaskState {
    root_task(snapshot)
        .and_then(|root| snapshot.tasks.get(&root).map(|task| task.state))
        .unwrap_or(crate::scheduler::core::model::task_graph::TaskState::Running)
}

/// Whether every execution of the run reached a terminal state: the run is no
/// longer *in flight*, so the slot it holds is free. The legacy engine's
/// equivalent is the terminal process event that evicts it from the resident
/// set. Deliberately weaker than the row projection's `settled`, which also
/// waits for the outbox and the channel deliveries: those keep the run's rows
/// alive, never its slot.
pub(super) fn run_settled(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
) -> bool {
    !snapshot.tasks.is_empty() && snapshot.tasks.values().all(|task| task.state.is_terminal())
}

/// The run's `__created` stamp (milliseconds): the FIFO key of the parked
/// queue. A run seeded before the fact existed reads as `0` and outranks
/// nothing.
pub(super) fn created_stamp(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
) -> i64 {
    root_variable(snapshot, concurrency::CREATED_VAR)
        .ok()
        .flatten()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(0)
}

/// The durable auto/waiting decision of one model node, from the import-time
/// map. A missing entry is an import inconsistency, not a default.
pub(super) fn auto_decision(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    node: &str,
) -> Result<bool> {
    let Some(raw) = root_variable(snapshot, model::AUTO_MAP_VAR)? else {
        return Err(runtime(
            "aggregate has no auto map; it predates the nested import",
        ));
    };
    let map: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&raw).map_err(runtime)?;
    match map.get(node) {
        Some(serde_json::Value::Bool(auto)) => Ok(*auto),
        other => Err(runtime(format!(
            "auto map has no decision for node '{node}': {other:?}"
        ))),
    }
}

/// The scope chain of one execution (nearest → root) merged nearest-wins
/// into name→json-string pairs: the variable space an effect's params and a
/// branch predicate resolve against.
///
/// Private (`__`-prefixed) variables are the engine's own aggregate facts —
/// the declaring model, the auto map, the run's owner, the cost start of a
/// timed-out execution — not the variable space a legacy task ever reads, so
/// they are left out (a `${{ __model }}` in a step's params must not resolve
/// where the legacy run would fail it).
///
/// One name is the exception: `__cost` is the legacy task *datum* the engine's
/// own `$cost()`/`$cost_in(...)` read (frozen by the tick that dispatched a
/// timeout branch), so it travels with the scope — a timeout branch's effect
/// params answer the same cost the legacy branch task did.
pub(crate) fn scope_vars(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    scope: &crate::scheduler::core::model::task_graph::TaskId,
) -> Vec<(String, String)> {
    let mut chain = vec![scope.clone()];
    let mut cursor = snapshot
        .tasks
        .get(scope)
        .and_then(|task| task.parent.clone());
    while let Some(parent) = cursor {
        cursor = snapshot
            .tasks
            .get(&parent)
            .and_then(|task| task.parent.clone());
        chain.push(parent);
    }
    let mut merged: Vec<(String, String)> = Vec::new();
    for scope in &chain {
        for ((key_scope, name), value) in &snapshot.variables {
            if key_scope == scope
                && (!crate::utils::consts::is_private_key(name)
                    || name == crate::utils::consts::TASK_COST)
                && !merged.iter().any(|(existing, _)| existing == name)
            {
                merged.push((name.clone(), value.clone()));
            }
        }
    }
    merged
}

/// The run's caller authority, from the durable root-scope variable the
/// import seeded — the legacy process env's `PROC_OWNER`. An aggregate that
/// predates the seed reads nothing, exactly like a process nobody sealed
/// ([`crate::ScopePolicy::default`] is `deny_all`).
pub(super) fn owner_of(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
) -> Result<crate::ScopePolicy> {
    match root_variable(snapshot, crate::utils::consts::PROC_OWNER)? {
        Some(raw) => serde_json::from_str(&raw).map_err(runtime),
        None => Ok(crate::ScopePolicy::default()),
    }
}

/// Whether a scope variable is an *engine* fact rather than workflow data.
///
/// Private (`__`-prefixed) names are the engine's by convention; `auto` is the
/// older spelling of the durable auto/waiting decision the seed and every
/// fan-out write into the execution's own scope (it predates the convention,
/// and the aggregate's variable map is keyed by name, so renaming it would
/// strand the decisions of every persisted aggregate). Engine facts are not
/// part of a run's inputs or outputs: a model's `inputs`/`exposes` schema must
/// judge the workflow's own data, exactly as the legacy read model did (its
/// task data never carried them).
pub(super) fn is_engine_fact(name: &str) -> bool {
    crate::utils::consts::is_private_key(name) || name == "auto"
}

/// Whether a scope variable is one of the engine's *failure* facts: the names
/// the catch fan-out reserves and writes into a failed step's scope so a
/// handler's guard can select on them (`$ecode()`/`message` are the legacy
/// task-data names, `error.code`/`error.message` the kernel's own). They are
/// engine facts like `auto` — the completion of a run that recovered must not
/// report its own error names as workflow data (the legacy read model kept
/// them on the task, out of a declared `exposes`).
pub(super) fn is_error_fact(name: &str) -> bool {
    matches!(name, "ecode" | "message" | "error.code" | "error.message")
}

/// The user variables of one run's root scope (the aggregate's JSON text
/// decoded), filtered by `keep`. Private names and the durable engine facts
/// ([`is_engine_fact`]) are never part of a run's data.
pub(super) fn root_variables(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    root: &crate::scheduler::core::model::task_graph::TaskId,
    keep: impl Fn(&str) -> bool,
) -> crate::Vars {
    let mut vars = crate::Vars::new();
    for ((scope, name), raw) in &snapshot.variables {
        if scope != root || is_engine_fact(name) || !keep(name) {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) {
            vars.set(name, value);
        }
    }
    vars
}

/// One run's declared `exposes` answered over its own scope — the legacy
/// `Task::outputs` projection, which *is* a workflow's output surface
/// (`exposes` filters what completes as outputs; it is not merely a schema for
/// everything the run happens to hold).
///
/// An entry answers by its own `value`: a `${{ }}` template is evaluated
/// against the run's variables, a literal is taken as it stands, and an entry
/// that declares no value at all reads the run's variable of that name (the
/// legacy `fill_outputs` rules). A name that resolves to nothing completes as
/// `null` rather than failing the run.
pub(super) fn exposed_variables(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    root: &crate::scheduler::core::model::task_graph::TaskId,
    exposes: &[crate::Variant],
) -> crate::Vars {
    let scope = scope_vars(snapshot, root);
    let read = |name: &str| {
        scope
            .iter()
            .find(|(key, _)| key == name)
            .and_then(|(_, raw)| serde_json::from_str::<serde_json::Value>(raw).ok())
    };
    let mut outputs = crate::Vars::new();
    for expose in exposes {
        let value = match &expose.value {
            serde_json::Value::String(text) => match crate::utils::get_expr(text) {
                Some(expr) => evaluate_expression(&expr, &scope).unwrap_or(serde_json::Value::Null),
                None => expose.value.clone(),
            },
            serde_json::Value::Null => read(&expose.name).unwrap_or(serde_json::Value::Null),
            value => value.clone(),
        };
        outputs.set(&expose.name, value);
    }
    outputs
}

/// Evaluate one `${{ }}` expression against a scope chain (nearest first), the
/// same variable space a branch predicate reads. `None` when the expression
/// does not compile, names something the scope does not hold, or has no JSON
/// value — the legacy projection completed such an entry as `null`.
fn evaluate_expression(expr: &str, scope: &[(String, String)]) -> Option<serde_json::Value> {
    let program = acts_expr::Expr::compile(expr).ok()?;
    let mut context = acts_expr::Context::new();
    for name in program.references() {
        let (_, raw) = scope.iter().find(|(key, _)| key == name)?;
        let value: serde_json::Value = serde_json::from_str(raw).ok()?;
        context.set(name, acts_expr::Value::from(value));
    }
    program.eval(&context).ok()?.to_json().ok()
}

/// The engine `Message` of one run's lifecycle: the workflow-level fact the
/// legacy `root.create_message` produced, built from the aggregate (its only
/// source). The root scope's user variables are the run's inputs; its outputs
/// are the model's `exposes` ([`exposed_variables`]) — the run's whole variable
/// space only when it declares none ([`root_variables`]).
pub(super) fn process_message(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    root: &crate::scheduler::core::model::task_graph::TaskId,
) -> crate::Message {
    let model = model_of(snapshot).ok().flatten();
    let inputs = root_variables(snapshot, root, |_| true);
    let outputs = match &model {
        Some(model) if !model.exposes.is_empty() => {
            exposed_variables(snapshot, root, &model.exposes)
        }
        // No `exposes`: the run's whole variable space is its output surface,
        // minus the failure vocabulary the catch fan-out pins on a step
        // ([`is_error_fact`]) — a run that recovered from a caught failure must
        // not report its own error names as data.
        _ => root_variables(snapshot, root, |name| !is_error_fact(name)),
    };
    let mid = model.map(|model| model.id).unwrap_or_default();
    let state = snapshot
        .tasks
        .get(root)
        .map(|task| lifecycle_state(task.state))
        .unwrap_or(crate::MessageState::None);
    crate::Message {
        id: crate::utils::longid(),
        tid: crate::utils::consts::TASK_ROOT_TID.to_string(),
        r#type: "workflow".to_string(),
        state,
        pid: snapshot.process_id.clone(),
        nid: crate::utils::consts::TASK_ROOT_TID.to_string(),
        mid,
        inputs,
        outputs,
        timestamp: crate::utils::time::time_millis(),
        ..Default::default()
    }
}

/// A kernel execution state in the engine's message vocabulary.
pub(super) fn lifecycle_state(
    state: crate::scheduler::core::model::task_graph::TaskState,
) -> crate::MessageState {
    use crate::MessageState as M;
    use crate::scheduler::core::model::task_graph::TaskState as S;
    match state {
        S::Completed => M::Completed,
        S::Submitted => M::Submitted,
        S::Backed => M::Backed,
        S::Cancelled => M::Cancelled,
        S::Aborted => M::Aborted,
        S::Skipped => M::Skipped,
        S::Failed => M::Error,
        S::Removed => M::Removed,
        S::Created | S::Ready | S::Running | S::Waiting => M::None,
    }
}

/// The failed root's error variables (`error.code` / `error.message`).
pub(super) fn root_error(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    root: &crate::scheduler::core::model::task_graph::TaskId,
) -> (String, String) {
    // The value is read the way every other scope reader reads it
    // ([`json_text`]): the failure vocabulary reaches the root both as an
    // engine-encoded JSON string (the catch fan-out's `json_var`) and as the
    // raw text a client's `ecode`/`message` options carry (`ApplyActionCommand`
    // writes the client's strings verbatim), so a strict decode would report
    // every client failure as an empty code.
    let read = |name: &str| {
        snapshot
            .variables
            .get(&(root.clone(), name.to_string()))
            .map(|raw| json_text(raw))
            .unwrap_or_default()
    };
    (read("error.code"), read("error.message"))
}

/// The private execution variables the start operation pins on a `uses` step: its
/// resolved params and its declared options. Every later message record
/// (completion, failure, skip) carries the facts the *start* operation resolved
/// instead of re-running the headless scope, whose snapshot seal may fail
/// (and must fail the act at start, never a later operation).
pub(super) const PARAMS_VAR: &str = "__params";
pub(super) const OPTIONS_VAR: &str = "__options";

/// The private execution variable a fan-out pins on a branch whose predicate
/// could not be evaluated (`{"code":..,"message":..}`): the branch is admitted
/// and fails on its first operation, exactly like the legacy branch task erroring in
/// its `init`.
pub(super) const BRANCH_ERROR_VAR: &str = "__branch_error";

/// One decided execution failure: the error variables a failure operation commits,
/// plus the legacy task-data name a catch predicate reads.
pub(super) struct Failure {
    /// The kernel's error code (`error.code`).
    pub(super) code: String,
    pub(super) message: String,
    /// The legacy `ecode` the failed act's data carried — what `$ecode()`
    /// answers inside a catch branch, which is the only thing that selects a
    /// handler. Equal to [`Self::code`] for a client error and for a branch
    /// predicate failure; a package error keeps the kernel's `EPACKAGE` code
    /// in `code` and the originating `ActError`'s own ecode here (`""` for
    /// everything but an `Exception`).
    pub(super) ecode: String,
}

impl Failure {
    /// A failure whose legacy `ecode` is the kernel's own code (client
    /// actions and branch predicate failures carry the same value in both
    /// vocabularies).
    pub(super) fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        let code = code.into();
        Self {
            ecode: code.clone(),
            code,
            message: message.into(),
        }
    }
}

/// One variable value in the aggregate's representation: JSON text. A raw
/// string would not decode into the headless scope a predicate evaluates on.
pub(super) fn json_var(value: &str) -> String {
    serde_json::Value::String(value.to_string()).to_string()
}

/// One variable of one execution scope, raw (the aggregate's own text).
pub(super) fn task_variable<'a>(
    snapshot: &'a crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    task: &crate::scheduler::core::model::task_graph::TaskId,
    name: &str,
) -> Option<&'a str> {
    snapshot
        .variables
        .get(&(task.clone(), name.to_string()))
        .map(String::as_str)
}

/// The resolved act facts of one `uses` execution.
#[derive(Clone)]

pub(super) struct ActFacts {
    pub(super) params: serde_json::Value,
    pub(super) options: serde_json::Value,
}

/// The act facts the start operation pinned on `execution`, when a runner resolved
/// them at start.
pub(super) fn act_facts(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    task: &crate::scheduler::core::model::task_graph::TaskId,
) -> Option<ActFacts> {
    let read = |name: &str| {
        snapshot
            .variables
            .get(&(task.clone(), name.to_string()))
            .map(|raw| {
                serde_json::from_str::<serde_json::Value>(raw).unwrap_or(serde_json::Value::Null)
            })
    };
    Some(ActFacts {
        params: read(PARAMS_VAR)?,
        options: read(OPTIONS_VAR).unwrap_or(serde_json::Value::Null),
    })
}

/// One variable value that holds a JSON string, decoded (`raw` when it is not
/// a JSON string).
pub(super) fn json_text(raw: &str) -> String {
    serde_json::from_str::<String>(raw).unwrap_or_else(|_| raw.to_string())
}

/// The outbox topic that announces one decided execution state — the act
/// message the legacy engine emitted for that transition.
pub(super) fn message_topic(
    state: crate::scheduler::core::model::task_graph::TaskState,
) -> Option<&'static str> {
    use crate::scheduler::core::model::task_graph::TaskState as S;
    match state {
        S::Completed => Some("task.completed"),
        S::Submitted => Some("task.submitted"),
        S::Backed => Some("task.backed"),
        S::Cancelled => Some("task.cancelled"),
        S::Failed => Some("task.failed"),
        S::Aborted => Some("task.aborted"),
        S::Skipped => Some("task.skipped"),
        S::Removed => Some("task.removed"),
        S::Created | S::Ready | S::Running | S::Waiting => None,
    }
}

/// The scope variables a legacy act message carries as `inputs`: the
/// predecessor's own (its outputs) first, the acting execution's own on top —
/// the legacy `Task::inputs` merges the node's vars last, so they win.
pub(super) fn input_vars(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    task: &crate::scheduler::core::model::task_graph::TaskId,
) -> serde_json::Map<String, serde_json::Value> {
    let mut inputs = serde_json::Map::new();
    let mut merge = |scope: &crate::scheduler::core::model::task_graph::TaskId| {
        for ((key_scope, name), raw) in &snapshot.variables {
            if key_scope != scope || is_engine_fact(name) {
                continue;
            }
            inputs.insert(
                name.clone(),
                serde_json::from_str(raw).unwrap_or(serde_json::Value::String(raw.clone())),
            );
        }
    };
    if let Some(task) = snapshot.tasks.get(task) {
        for predecessor in &task.predecessors {
            merge(predecessor);
        }
    }
    merge(task);
    inputs
}

/// Validate the run's `exposes` against the message's outputs, like the legacy
/// `on_proc` handler; `Some(text)` is the legacy error text.
pub(super) fn exposes_error(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    message: &crate::Message,
) -> Option<String> {
    let model = model_of(snapshot).ok().flatten()?;
    if model.exposes.is_empty() {
        return None;
    }
    let schema = crate::ActSchema::Multiple(model.exposes.clone());
    schema
        .validate(&message.outputs.to_value())
        .err()
        .map(|error| format!("model({}) outputs validation error: {}", model.id, error))
}

/// One branch predicate, evaluated against the aggregate's variables the way
/// the legacy `ctx.eval` resolves them: the scope chain of the fanning-out
/// execution, nearest scope wins, with the completing action's inputs on top
/// (legacy writes the action's vars into the task scope before its branches
/// evaluate). A compile failure, an unresolved name or a non-boolean result
/// is an `Invalid` predicate — an error, never a silent `False` (the legacy
/// engine fails the task on the same conditions).
pub(super) fn evaluate_predicate(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    scope: &crate::scheduler::core::model::task_graph::TaskId,
    expr: &str,
    overlay: &[(String, String)],
) -> PredicateValue {
    let program = match acts_expr::Expr::compile(expr) {
        Ok(program) => program,
        Err(error) => {
            return PredicateValue::Invalid {
                message: error.to_string(),
            };
        }
    };
    // The variable space: the execution's scope chain, nearest wins, with
    // the completing action's inputs on top.
    let merged = scope_vars(snapshot, scope);
    let resolve = |name: &str| -> Option<serde_json::Value> {
        if let Some((_, raw)) = overlay.iter().find(|(key, _)| key == name)
            && let Ok(value) = serde_json::from_str::<serde_json::Value>(raw)
        {
            return Some(value);
        }
        merged
            .iter()
            .find(|(key, _)| key == name)
            .and_then(|(_, raw)| serde_json::from_str::<serde_json::Value>(raw).ok())
    };
    let mut context = acts_expr::Context::new();
    for name in program.references() {
        match resolve(name) {
            Some(value) => {
                context.set(name, acts_expr::Value::from(value));
            }
            None => {
                return PredicateValue::Invalid {
                    message: format!("variable '{name}' is not set where branch '{expr}' runs"),
                };
            }
        }
    }
    match program.eval(&context) {
        Ok(value) if value.is_null() => PredicateValue::Null,
        Ok(value) => match value.as_bool() {
            Some(true) => PredicateValue::True,
            Some(false) => PredicateValue::False,
            None => PredicateValue::Invalid {
                message: format!("branch predicate '{expr}' must evaluate to a boolean"),
            },
        },
        Err(error) => PredicateValue::Invalid {
            message: error.to_string(),
        },
    }
}

/// The message payload's `outputs` bag: the acting execution's own scope writes
/// minus the private engine facts and the error variables (the legacy
/// `Task::outputs`). An operation's plan may carry writes for other scopes too (the
/// children a fan-out admitted), which are not this message's outputs.
pub(super) fn message_outputs(
    outputs: &[VariableWrite],
    task: &crate::scheduler::core::model::task_graph::TaskId,
) -> serde_json::Value {
    let vars: serde_json::Map<String, serde_json::Value> = outputs
        .iter()
        .filter(|write| {
            write.scope == *task
                && !crate::utils::consts::is_private_key(&write.name)
                && !write.name.starts_with("error.")
        })
        .map(|write| {
            (
                write.name.clone(),
                serde_json::from_str(&write.value).unwrap_or(serde_json::Value::Null),
            )
        })
        .collect();
    serde_json::Value::Object(vars)
}

/// The legacy node kind and node name of one model node: what a `Message`'s
/// `type`/`name` carried (`Task::create_message` reads `node.kind()` and
/// `node.content.name()`). The kernel's own model nodes are steps, branches and
/// the workflow root; an act is a `uses` step, whose message takes the act
/// payload path instead.
pub(super) fn node_kind_and_name(
    model: &crate::model::Workflow,
    node: &str,
) -> (&'static str, String) {
    if node == crate::utils::consts::TASK_ROOT_TID {
        return ("workflow", model.name.clone());
    }
    for step in &model.steps {
        if let Some(name) = branch_name(step, node) {
            return ("branch", name);
        }
    }
    (
        "step",
        find_step_in_model(model, node)
            .map(|step| step.name.clone())
            .unwrap_or_default(),
    )
}

/// The name of the branch with this id anywhere under one step, if any.
pub(super) fn branch_name(step: &crate::model::Step, target: &str) -> Option<String> {
    for branch in model::step_branches(step).unwrap_or_default() {
        if branch.id == target {
            return Some(branch.name.clone());
        }
        for inner in &branch.steps {
            if let Some(name) = branch_name(inner, target) {
                return Some(name);
            }
        }
    }
    for catch in &step.catches {
        if let Some(name) = branch_name(catch, target) {
            return Some(name);
        }
    }
    for timeout in &step.timeouts {
        if let Some(name) = branch_name(timeout, target) {
            return Some(name);
        }
    }
    None
}
