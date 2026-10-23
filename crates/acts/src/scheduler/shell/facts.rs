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

/// The root execution of an aggregate — the core decision's single
/// derivation (see [`crate::scheduler::core::decision::root_task`]).
pub(super) fn root_task(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
) -> Option<crate::scheduler::core::model::task_graph::TaskId> {
    crate::scheduler::core::decision::root_task(snapshot)
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

/// The durable `Msg` run-bound decision of one model node, from the
/// import-time map: a node whose package runs as `Msg` is delivered once —
/// its completion record — so its start announces no `Created` message. A
/// missing entry is an import inconsistency, not a default.
pub(super) fn msg_decision(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    node: &str,
) -> Result<bool> {
    let Some(raw) = root_variable(snapshot, model::MSG_MAP_VAR)? else {
        return Err(runtime(
            "aggregate has no msg map; it predates the nested import",
        ));
    };
    let map: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&raw).map_err(runtime)?;
    match map.get(node) {
        Some(serde_json::Value::Bool(msg)) => Ok(*msg),
        other => Err(runtime(format!(
            "msg map has no decision for node '{node}': {other:?}"
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
///
/// The param fan-out's per-item bindings are the other exception, in the
/// opposite direction: they are stored privately
/// (`FANOUT_INDEX_VAR`/`FANOUT_VALUE_VAR`) so they can never fold into an
/// ancestor scope or reach a row, a run's outputs or a message's `inputs`, but
/// inside the item's own scope they answer as `index`/`value` — the names the
/// container packages' inline acts read. The alias is a fallback: a workflow
/// variable of the same name in this or a nearer scope wins.
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
        // The fan-out's private item facts, under the public names the
        // templates' acts read. Only when the name is not already answered by
        // this or a nearer scope: an explicit workflow variable wins.
        for (private, public) in [
            (crate::utils::consts::FANOUT_INDEX_VAR, "index"),
            (crate::utils::consts::FANOUT_VALUE_VAR, "value"),
        ] {
            if merged.iter().any(|(existing, _)| existing == public) {
                continue;
            }
            if let Some(raw) = snapshot
                .variables
                .get(&(scope.clone(), private.to_string()))
            {
                merged.push((public.to_string(), raw.clone()));
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
        // `tid` is the root instance's wire (C5); `nid` stays the model root
        // node (`$`), the split every other message carries.
        tid: root.wire(&snapshot.process_id),
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
#[derive(Debug)]
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

/// What one model node says in the two vocabularies it speaks, plus the name
/// both carry.
///
/// `declared_kind` is the node's declaration ([`crate::NodeKind`]): the read
/// model pins this word, so a `uses` step's row stays `step`
/// (`projection::project_task`) and the import's virtual container is `block`
/// — the one word that is not a declaration's. `message_kind` is the message
/// level C6 fixed: the same `uses` step is `act` (the act payload path), a
/// plain step is `step`, the root is `workflow`, and a branch announces its
/// terminal states as `branch` — a virtual container as `block` — like any
/// other execution (the message stream is the flow trace). The two words
/// deliberately differ for exactly one shape — the row contract must not be
/// read as a misspelling of the message level (C6, §10).
pub(super) struct NodeFacts {
    pub(super) declared_kind: crate::NodeKind,
    pub(super) message_kind: crate::NodeKind,
    pub(super) name: String,
}

/// The single derivation of a model node's name and kinds. The row projection
/// takes `declared_kind`/`name`; the message payload takes
/// `message_kind`/`name` — one traversal, one name source (the root's name is
/// the workflow's `model.name`).
pub(super) fn node_facts(model: &crate::model::Workflow, node: &str) -> NodeFacts {
    use crate::NodeKind;
    if node == crate::utils::consts::TASK_ROOT_TID {
        return NodeFacts {
            declared_kind: NodeKind::Workflow,
            message_kind: NodeKind::Workflow,
            name: model.name.clone(),
        };
    }
    for step in &model.steps {
        if let Some((kind, name)) = branch_facts(step, node) {
            return NodeFacts {
                declared_kind: kind.clone(),
                message_kind: kind,
                name,
            };
        }
    }
    match find_step_in_model(model, node) {
        Some(step) => {
            let message_kind = match step.uses.as_deref() {
                Some(uses) if !uses.trim().is_empty() => NodeKind::Act,
                _ => NodeKind::Step,
            };
            NodeFacts {
                declared_kind: NodeKind::Step,
                message_kind,
                name: step.name.clone(),
            }
        }
        None => NodeFacts {
            declared_kind: NodeKind::Step,
            message_kind: NodeKind::Step,
            name: String::new(),
        },
    }
}

/// The kind and name of the branch with this id anywhere under one step, if
/// any. The import's virtual container (`Branch::r#virtual`) is kind `block`
/// — the declaration declares no such node — and every declared branch stays
/// `branch`.
pub(super) fn branch_facts(
    step: &crate::model::Step,
    target: &str,
) -> Option<(crate::NodeKind, String)> {
    use crate::NodeKind;
    for branch in model::step_branches(step).unwrap_or_default() {
        if branch.id == target {
            let kind = if branch.r#virtual {
                NodeKind::Block
            } else {
                NodeKind::Branch
            };
            return Some((kind, branch.name.clone()));
        }
        for inner in &branch.steps {
            if let Some(facts) = branch_facts(inner, target) {
                return Some(facts);
            }
        }
    }
    for catch in &step.catches {
        if let Some(facts) = branch_facts(catch, target) {
            return Some(facts);
        }
    }
    for timeout in &step.timeouts {
        if let Some(facts) = branch_facts(timeout, target) {
            return Some(facts);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NodeKind, Vars, model::Workflow};

    /// The two vocabularies are the legacy four words; a `uses` step is an
    /// act on the message level but stays a step in the row contract — the
    /// deliberate divergence C6 pinned. The name is the same in both.
    #[test]
    fn node_facts_name_the_row_and_message_kinds_of_each_node() {
        let model = Workflow::new()
            .with_id("wf")
            .with_name("orders")
            .with_step(|step| step.with_id("plain").with_name("Plain"))
            .with_step(|step| {
                step.with_id("call")
                    .with_name("Call")
                    .with_uses("test.irq", Vars::new())
            })
            .with_step(|step| {
                step.with_id("blank")
                    .with_name("Blank")
                    .with_uses("   ", Vars::new())
            })
            .with_step(|step| {
                step.with_id("branchy")
                    .with_name("Branchy")
                    .with_branch(|branch| branch.with_id("b1").with_name("Branch one"))
            })
            .with_step(|step| {
                step.with_id("blocky")
                    .with_name("Blocky")
                    .with_branch(|mut branch| {
                        branch.id = "blocky#acts".to_string();
                        branch.name = "Virtual".to_string();
                        branch.r#virtual = true;
                        branch
                    })
            });

        let facts = |node| {
            let facts = node_facts(&model, node);
            (facts.declared_kind, facts.message_kind, facts.name)
        };

        assert_eq!(
            facts(crate::utils::consts::TASK_ROOT_TID),
            (NodeKind::Workflow, NodeKind::Workflow, "orders".to_string())
        );
        assert_eq!(
            facts("plain"),
            (NodeKind::Step, NodeKind::Step, "Plain".to_string())
        );
        assert_eq!(
            facts("call"),
            (NodeKind::Step, NodeKind::Act, "Call".to_string()),
            "a uses step is a row 'step' but a message 'act' (C6)"
        );
        assert_eq!(
            facts("blank"),
            (NodeKind::Step, NodeKind::Step, "Blank".to_string()),
            "a blank uses is not a package: the act path filters it the same way"
        );
        assert_eq!(
            facts("b1"),
            (NodeKind::Branch, NodeKind::Branch, "Branch one".to_string())
        );
        assert_eq!(
            facts("blocky#acts"),
            (NodeKind::Block, NodeKind::Block, "Virtual".to_string()),
            "the import's virtual container speaks `block` in both vocabularies"
        );
        assert_eq!(
            facts("missing"),
            (NodeKind::Step, NodeKind::Step, String::new())
        );
    }
}

#[cfg(test)]
mod coverage_tests {
    use super::*;
    use crate::scheduler::core::commit::aggregate::ProcessSnapshot;
    use crate::scheduler::core::model::task_graph::{TaskId, TaskState};
    use crate::utils::consts;
    use crate::{MessageState, Vars, model::Workflow};

    fn seeded(workflow: &Workflow) -> ProcessSnapshot {
        ProcessSpec::from_workflow("p-facts", workflow, &|_| None)
            .unwrap()
            .seed_snapshot()
            .unwrap()
    }

    fn root() -> TaskId {
        TaskId::new(consts::TASK_ROOT_TID, 0).unwrap()
    }

    #[test]
    fn an_aggregate_without_the_auto_map_or_a_decision_is_an_error() {
        let workflow = Workflow::new().with_step(|step| step.with_id("s1"));
        let mut snapshot = seeded(&workflow);
        snapshot.variables.retain(|(scope, name), _| {
            !(scope == &root() && name == crate::scheduler::shell::model::AUTO_MAP_VAR)
        });
        let error = auto_decision(&snapshot, "s1").unwrap_err();
        assert!(format!("{error}").contains("no auto map"), "{error}");

        let snapshot = seeded(&workflow);
        let error = auto_decision(&snapshot, "ghost").unwrap_err();
        assert!(
            format!("{error}").contains("no decision for node 'ghost'"),
            "{error}"
        );
    }

    #[test]
    fn an_ownerless_aggregate_denies_everything() {
        let workflow = Workflow::new().with_step(|step| step.with_id("s1"));
        let mut snapshot = seeded(&workflow);
        snapshot
            .variables
            .retain(|(scope, name), _| !(scope == &root() && name == consts::PROC_OWNER));
        assert_eq!(owner_of(&snapshot).unwrap(), crate::ScopePolicy::default());
    }

    #[test]
    fn exposes_answer_templates_literals_and_names() {
        let workflow = Workflow::new()
            .with_var("v", 7)
            .with_var("named", "from-scope")
            .with_step(|step| step.with_id("s1"));
        let snapshot = seeded(&workflow);
        let variant = |name: &str, value: serde_json::Value| crate::Variant {
            name: name.to_string(),
            value,
            ..crate::Variant::default()
        };
        let exposes = vec![
            variant("templated", serde_json::json!("${{ v }}")),
            variant("literal", serde_json::json!("plain text")),
            variant("named", serde_json::Value::Null),
            variant("missing", serde_json::Value::Null),
            variant("given", serde_json::json!({"a": 1})),
        ];
        let outputs = exposed_variables(&snapshot, &root(), &exposes);
        assert_eq!(outputs.get::<i32>("templated"), Some(7));
        assert_eq!(
            outputs.get::<String>("literal").as_deref(),
            Some("plain text")
        );
        assert_eq!(
            outputs.get::<String>("named").as_deref(),
            Some("from-scope")
        );
        assert_eq!(
            outputs.get::<serde_json::Value>("missing"),
            Some(serde_json::Value::Null)
        );
        assert_eq!(
            outputs.get::<serde_json::Value>("given").unwrap()["a"],
            serde_json::json!(1)
        );
    }

    /// Every execution state has a message state: the terminal seven map onto
    /// their legacy names, the live four onto `None`.
    #[test]
    fn every_execution_state_maps_onto_a_message_state() {
        for (state, expected) in [
            (TaskState::Completed, MessageState::Completed),
            (TaskState::Submitted, MessageState::Submitted),
            (TaskState::Backed, MessageState::Backed),
            (TaskState::Cancelled, MessageState::Cancelled),
            (TaskState::Aborted, MessageState::Aborted),
            (TaskState::Skipped, MessageState::Skipped),
            (TaskState::Failed, MessageState::Error),
            (TaskState::Removed, MessageState::Removed),
            (TaskState::Created, MessageState::None),
            (TaskState::Ready, MessageState::None),
            (TaskState::Running, MessageState::None),
            (TaskState::Waiting, MessageState::None),
        ] {
            assert_eq!(lifecycle_state(state), expected, "{state:?}");
        }
    }

    /// A task absent from the aggregate still contributes its own (empty)
    /// scope; a missing execution is not a panic.
    #[test]
    fn input_vars_of_a_missing_execution_are_empty() {
        let workflow = Workflow::new().with_step(|step| step.with_id("s1"));
        let snapshot = seeded(&workflow);
        let ghost = TaskId::new("ghost", 0).unwrap();
        assert!(input_vars(&snapshot, &ghost).is_empty());
    }

    #[test]
    fn an_invalid_predicate_names_the_compile_failure() {
        let workflow = Workflow::new()
            .with_var("v", 1)
            .with_step(|step| step.with_id("s1"));
        let snapshot = seeded(&workflow);
        match evaluate_predicate(&snapshot, &root(), "v >", &[]) {
            crate::scheduler::core::plan::branch::PredicateValue::Invalid { message } => {
                assert!(!message.is_empty())
            }
            other => panic!("expected an invalid predicate: {other:?}"),
        }
    }

    #[test]
    fn a_non_boolean_predicate_is_invalid() {
        let workflow = Workflow::new()
            .with_var("v", 1)
            .with_step(|step| step.with_id("s1"));
        let snapshot = seeded(&workflow);
        match evaluate_predicate(&snapshot, &root(), "v + 1", &[]) {
            crate::scheduler::core::plan::branch::PredicateValue::Invalid { message } => {
                assert!(message.contains("must evaluate to a boolean"), "{message}")
            }
            other => panic!("expected an invalid predicate: {other:?}"),
        }
    }

    /// A timeout handler is a branch node like a `branches` entry: its name is
    /// resolved through `node_facts`.
    #[test]
    fn a_timeout_handler_resolves_as_a_branch_node() {
        let workflow = Workflow::new().with_step(|step| {
            step.with_id("s1")
                .with_uses("acts.transform.set", Vars::new())
                .with_timeout(|timeout| {
                    timeout.with_id("t1").with_branch(|branch| {
                        branch
                            .with_id("tb")
                            .with_name("Timeout branch")
                            .with_if("$cost() >= 1000")
                    })
                })
        });
        // The handler step itself is a step; its branch is the branch node.
        assert_eq!(
            node_facts(&workflow, "t1").declared_kind,
            crate::NodeKind::Step
        );
        let facts = node_facts(&workflow, "tb");
        assert_eq!(facts.declared_kind, crate::NodeKind::Branch);
        assert_eq!(facts.name, "Timeout branch");
    }
}
