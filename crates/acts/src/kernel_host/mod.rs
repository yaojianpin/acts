//! The kernel host: the async shell driving the deterministic kernel.
//!
//! It owns no state of its own — every fact lives in the aggregate documents
//! over the engine's [`KvStore`]. Wakes are a channel and strictly an
//! optimization: losing one is safe because `recover` re-derives the same
//! work from the recovery index. The host drives one operation kind per
//! durable hop:
//!
//! - `ExecuteTask` commits `Ready → Running` (the task started) and seeds the
//!   `ApplyAction` operation whose effect performs the task's work: the
//!   attached [`effects::EffectRunner`] runs the step's package (the serving
//!   host has [`PackageEffectRunner`]; a host without one auto-completes).
//! - `ApplyAction` commits `Running → Completed` (the effect is durable) and
//!   seeds the `PropagateTask` operation in the same atomic batch — a crash
//!   between effect and propagation leaves the operation open, never the
//!   reverse.
//! - `PropagateTask` re-plans propagation from the authoritative snapshot and
//!   commits the successors' `Created → Ready` plus their `ExecuteTask`
//!   operations; join readiness omits waiting successors idempotently.
//!
//! Every hop is one `commit_transition` with the hop's own operation as the
//! `source_operation`, so the operation is committed inside the same atomic
//! batch as its business effect. When a hop's durable remainder already
//! exists (a wake redelivered after the commit), the hop commits an empty
//! plan sourced by its own operation, which completes the operation without
//! touching business state.

use crate::kernel::{
    AsyncAggregateRepository, AsyncRecoveryRepository, BranchCandidate, BranchCommand,
    BranchDecision, BranchPlanner, Clock, ContainerChild, ContainerChildPlan,
    ContainerFanoutCommand, ContainerPredecessor, DeliveryClaim, KvDocumentStore, OperationCommit,
    OperationEvent, OperationId, OperationIntent, OperationKind, OperationState, OutboxEvent,
    PredicateBasis, PredicateValue, PropagateCommand, PropagationPlanner, RecoveryDisposition,
    RecoveryPlanner, RecoveryReport, StateChange, SystemClock, TransitionPlan, VariableWrite,
};
use crate::store::{KvStore, MemoryStore};
use crate::{ActError, Result};
use std::sync::Arc;
use tokio::sync::mpsc::{Receiver, Sender, channel};

mod effects;
mod message;
mod model;
mod predicate;
mod projection;
mod scope;

use self::model::execution_graph;
pub(crate) use effects::PackageEffectRunner;
pub(crate) use message::MessageForwarder;
pub(crate) use model::ProcessSpec;
pub(crate) use predicate::EnginePredicateRunner;
pub(crate) use projection::RowProjector;

const OWNER: &str = "kernel-host";
const LEASE_MS: i64 = 60_000;
/// How many times a client action re-plans after losing a compare-and-swap:
/// the serving loop commits hops of the same aggregate concurrently, so a
/// client's plan can go stale before its commit. A retry re-reads the
/// snapshot; the action then either applies or is legitimately refused.
const ACTION_ATTEMPTS: usize = 8;
/// The error code a lost compare-and-swap carries, so a retryable race is
/// distinguishable from a caller-correctable refusal.
const CONFLICT_ECODE: &str = "ECONFLICT";
const DELIVERY_LEASE_MS: i64 = 30_000;
const PAGE_SIZE: usize = 128;

fn runtime(error: impl std::fmt::Display) -> ActError {
    ActError::Runtime(error.to_string())
}

/// A lost compare-and-swap: retryable, unlike a caller-correctable refusal.
fn conflict(message: impl Into<String>) -> ActError {
    ActError::Exception {
        ecode: CONFLICT_ECODE.to_string(),
        message: message.into(),
    }
}

/// Whether an error is a lost compare-and-swap (see [`conflict`]).
fn is_conflict(error: &ActError) -> bool {
    matches!(error, ActError::Exception { ecode, .. } if ecode == CONFLICT_ECODE)
}

/// Map a repository failure: a *stale plan* — the aggregate moved between the
/// load a plan was formed from and its commit — is the same retryable race as
/// a lost compare-and-swap, so a caller can re-plan against the fresh
/// snapshot. Everything else is an error.
fn repository_error(error: crate::kernel::AsyncRepositoryError) -> ActError {
    match &error {
        crate::kernel::AsyncRepositoryError::Domain(
            crate::kernel::CommitError::VersionConflict { .. },
        ) => conflict(format!("stale plan: {error}")),
        _ => runtime(error),
    }
}

/// The declaring model of a snapshot, read from the durable root-scope
/// variable. `None` only for aggregates seeded before the nested import.
fn model_of(snapshot: &crate::kernel::AggregateSnapshot) -> Result<Option<crate::model::Workflow>> {
    let Some(raw) = root_variable(snapshot, model::MODEL_VAR)? else {
        return Ok(None);
    };
    serde_json::from_str::<crate::model::Workflow>(&raw)
        .map(Some)
        .map_err(runtime)
}

/// The root execution of an aggregate: the `$` node's earliest generation —
/// the generation a re-entry never removes.
fn root_execution(
    snapshot: &crate::kernel::AggregateSnapshot,
) -> Option<crate::kernel::ExecutionId> {
    snapshot
        .executions
        .keys()
        .filter(|id| id.task_id == crate::utils::consts::TASK_ROOT_TID)
        .min_by_key(|id| id.generation)
        .cloned()
}

/// One root-scope variable (root = the `$` execution, earliest generation —
/// the generation a re-entry never removes).
fn root_variable(
    snapshot: &crate::kernel::AggregateSnapshot,
    name: &str,
) -> Result<Option<String>> {
    Ok(root_execution(snapshot)
        .and_then(|root| snapshot.variables.get(&(root, name.to_string())).cloned()))
}

/// The durable auto/waiting decision of one model node, from the import-time
/// map. A missing entry is an import inconsistency, not a default.
fn auto_decision(snapshot: &crate::kernel::AggregateSnapshot, node: &str) -> Result<bool> {
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
    snapshot: &crate::kernel::AggregateSnapshot,
    scope: &crate::kernel::ExecutionId,
) -> Vec<(String, String)> {
    let mut chain = vec![scope.clone()];
    let mut cursor = snapshot
        .executions
        .get(scope)
        .and_then(|execution| execution.parent.clone());
    while let Some(parent) = cursor {
        cursor = snapshot
            .executions
            .get(&parent)
            .and_then(|execution| execution.parent.clone());
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
fn owner_of(snapshot: &crate::kernel::AggregateSnapshot) -> Result<crate::ScopePolicy> {
    match root_variable(snapshot, crate::utils::consts::PROC_OWNER)? {
        Some(raw) => serde_json::from_str(&raw).map_err(runtime),
        None => Ok(crate::ScopePolicy::default()),
    }
}

/// The engine `Message` of one run's lifecycle: the workflow-level fact the
/// legacy `root.create_message` produced, built from the aggregate (its only
/// source). The root scope's user variables are the run's outputs.
fn process_message(
    snapshot: &crate::kernel::AggregateSnapshot,
    root: &crate::kernel::ExecutionId,
) -> crate::Message {
    let mut vars = crate::Vars::new();
    for ((scope, name), raw) in &snapshot.variables {
        if scope != root || crate::utils::consts::is_private_key(name) {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) {
            vars.set(name, value);
        }
    }
    let mid = model_of(snapshot)
        .ok()
        .flatten()
        .map(|model| model.id)
        .unwrap_or_default();
    let state = snapshot
        .executions
        .get(root)
        .map(|execution| lifecycle_state(execution.state))
        .unwrap_or(crate::MessageState::None);
    crate::Message {
        id: crate::utils::longid(),
        tid: crate::utils::consts::TASK_ROOT_TID.to_string(),
        r#type: "workflow".to_string(),
        state,
        pid: snapshot.process_id.clone(),
        nid: crate::utils::consts::TASK_ROOT_TID.to_string(),
        mid,
        inputs: vars.clone(),
        outputs: vars,
        timestamp: crate::utils::time::time_millis(),
        ..Default::default()
    }
}

/// A kernel execution state in the engine's message vocabulary.
fn lifecycle_state(state: crate::kernel::ExecutionState) -> crate::MessageState {
    use crate::MessageState as M;
    use crate::kernel::ExecutionState as S;
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
fn root_error(
    snapshot: &crate::kernel::AggregateSnapshot,
    root: &crate::kernel::ExecutionId,
) -> (String, String) {
    let read = |name: &str| {
        snapshot
            .variables
            .get(&(root.clone(), name.to_string()))
            .and_then(|raw| serde_json::from_str::<String>(raw).ok())
            .unwrap_or_default()
    };
    (read("error.code"), read("error.message"))
}

/// The private execution variables the start hop pins on a `uses` step: its
/// resolved params and its declared options. Every later message record
/// (completion, failure, skip) carries the facts the *start* hop resolved
/// instead of re-running the headless scope, whose snapshot seal may fail
/// (and must fail the act at start, never a later hop).
const PARAMS_VAR: &str = "__params";
const OPTIONS_VAR: &str = "__options";

/// The private execution variable a fan-out pins on a branch whose predicate
/// could not be evaluated (`{"code":..,"message":..}`): the branch is admitted
/// and fails on its first hop, exactly like the legacy branch task erroring in
/// its `init`.
const BRANCH_ERROR_VAR: &str = "__branch_error";

/// One decided execution failure: the error variables a failure hop commits,
/// plus the legacy task-data name a catch predicate reads.
struct Failure {
    /// The kernel's error code (`error.code`).
    code: String,
    message: String,
    /// The legacy `ecode` the failed act's data carried — what `$ecode()`
    /// answers inside a catch branch, which is the only thing that selects a
    /// handler. Equal to [`Self::code`] for a client error and for a branch
    /// predicate failure; a package error keeps the kernel's `EPACKAGE` code
    /// in `code` and the originating `ActError`'s own ecode here (`""` for
    /// everything but an `Exception`).
    ecode: String,
}

impl Failure {
    /// A failure whose legacy `ecode` is the kernel's own code (client
    /// actions and branch predicate failures carry the same value in both
    /// vocabularies).
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
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
fn json_var(value: &str) -> String {
    serde_json::Value::String(value.to_string()).to_string()
}

/// One variable of one execution scope, raw (the aggregate's own text).
fn execution_variable<'a>(
    snapshot: &'a crate::kernel::AggregateSnapshot,
    execution: &crate::kernel::ExecutionId,
    name: &str,
) -> Option<&'a str> {
    snapshot
        .variables
        .get(&(execution.clone(), name.to_string()))
        .map(String::as_str)
}

/// What one timeout dispatch round decided (see
/// [`KernelHost::timeout_round`]).
enum TimeoutRound {
    /// The due branch and its decided guard: the hop dispatches it.
    Dispatch {
        branch: crate::kernel::TimeoutBranch,
        guard: TimeoutGuard,
        cost_ms: i64,
        /// How many branches the node declares: the hop re-arms the timer while
        /// the one-shot marker is shorter than that.
        declared: usize,
    },
    /// Something can still fire, but not now: the guard's window is not open
    /// (the legacy branch's `init` skipping without claiming) or the first
    /// un-fired branch is in flight. The timer stays armed.
    Idle,
    /// Nothing can fire any more: the owner is decided, its node declares no
    /// `timeouts`, or every branch already fired.
    Retire,
}

/// One branch guard's verdict.
enum TimeoutGuard {
    /// The guard holds (or the branch declares none): dispatch the branch.
    Fire,
    /// The guard could not be evaluated (a non-boolean value or a compile/eval
    /// error): the branch is admitted and fails on its first hop, exactly like
    /// the legacy branch task whose `init` errored. The message is the pinned
    /// failure.
    Fail(String),
}

/// The next free operation sequence on one execution, counting both the
/// snapshot's records and the intents this plan already claims: a hop that adds
/// its own timer next to the act's completion op must not collide with it.
fn next_plan_sequence(
    snapshot: &crate::kernel::AggregateSnapshot,
    plan: &TransitionPlan,
    task_id: &str,
    generation: u64,
) -> u64 {
    let from_snapshot = snapshot.next_operation_sequence(task_id, generation);
    let from_plan = plan
        .operations
        .iter()
        .filter(|intent| intent.id.task_id == task_id && intent.id.generation == generation)
        .map(|intent| intent.id.sequence.saturating_add(1))
        .max()
        .unwrap_or(0);
    from_snapshot.max(from_plan)
}

/// Record that one execution parked in `Waiting` for its children: the mark
/// [`crate::kernel::ContainerPlanner::plan_join`] reads. `Waiting` alone is
/// ambiguous — an execution waiting for its own act (the legacy `Interrupt`)
/// carries it too — and a child's settlement must not decide the latter.
fn mark_parked_for_children(plan: &mut TransitionPlan, execution: &crate::kernel::ExecutionId) {
    plan.variable_writes
        .retain(|write| !(write.scope == *execution && write.name == crate::kernel::PARK_VAR));
    plan.variable_writes.push(VariableWrite {
        scope: execution.clone(),
        name: crate::kernel::PARK_VAR.to_string(),
        value: crate::kernel::PARK_CHILDREN.to_string(),
    });
}

/// Whether this execution is a step deciding a caught error: its failure hop
/// pinned the ERROR sign (the legacy `Step::sign() == Sign::ERROR`), which the
/// catch join answers and no later hop may override.
fn is_catch_decider(
    snapshot: &crate::kernel::AggregateSnapshot,
    execution: &crate::kernel::ExecutionId,
) -> bool {
    crate::kernel::execution_sign(snapshot, execution) == Some(crate::kernel::SIGN_ERROR)
}

/// The failure one package error reports: the kernel's own code and the
/// legacy `ecode` the failed act's data carried. The legacy engine wrote
/// `Error::from(ActError)` into the task data (`Task::set_err`), so only an
/// `Exception` carries a code and every other package error reads empty
/// through `$ecode()`.
fn package_failure(error: &crate::ActError) -> Failure {
    Failure {
        code: "EPACKAGE".to_string(),
        message: format!("{error}"),
        ecode: match error {
            crate::ActError::Exception { ecode, .. } => ecode.clone(),
            _ => String::new(),
        },
    }
}

/// The branch predicate failure a fan-out pinned on `execution`, if any.
fn branch_error(
    snapshot: &crate::kernel::AggregateSnapshot,
    execution: &crate::kernel::ExecutionId,
) -> Option<Failure> {
    let raw = snapshot
        .variables
        .get(&(execution.clone(), BRANCH_ERROR_VAR.to_string()))?;
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    Some(Failure::new(
        value
            .get("code")
            .and_then(|code| code.as_str())
            .unwrap_or_default()
            .to_string(),
        value
            .get("message")
            .and_then(|message| message.as_str())
            .unwrap_or_default()
            .to_string(),
    ))
}

/// The resolved act facts of one `uses` execution.
#[derive(Clone)]
struct ActFacts {
    params: serde_json::Value,
    options: serde_json::Value,
}

/// The act facts the start hop pinned on `execution`, when a runner resolved
/// them at start.
fn act_facts(
    snapshot: &crate::kernel::AggregateSnapshot,
    execution: &crate::kernel::ExecutionId,
) -> Option<ActFacts> {
    let read = |name: &str| {
        snapshot
            .variables
            .get(&(execution.clone(), name.to_string()))
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
fn json_text(raw: &str) -> String {
    serde_json::from_str::<String>(raw).unwrap_or_else(|_| raw.to_string())
}

/// The outbox topic that announces one decided execution state — the act
/// message the legacy engine emitted for that transition.
fn message_topic(state: crate::kernel::ExecutionState) -> Option<&'static str> {
    use crate::kernel::ExecutionState as S;
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
fn input_vars(
    snapshot: &crate::kernel::AggregateSnapshot,
    execution: &crate::kernel::ExecutionId,
) -> serde_json::Map<String, serde_json::Value> {
    let mut inputs = serde_json::Map::new();
    let mut merge = |scope: &crate::kernel::ExecutionId| {
        for ((key_scope, name), raw) in &snapshot.variables {
            if key_scope != scope || crate::utils::consts::is_private_key(name) || name == "auto" {
                continue;
            }
            inputs.insert(
                name.clone(),
                serde_json::from_str(raw).unwrap_or(serde_json::Value::String(raw.clone())),
            );
        }
    };
    if let Some(execution) = snapshot.executions.get(execution) {
        for predecessor in &execution.predecessors {
            merge(predecessor);
        }
    }
    merge(execution);
    inputs
}

/// Validate the run's `exposes` against the message's outputs, like the legacy
/// `on_proc` handler; `Some(text)` is the legacy error text.
fn exposes_error(
    snapshot: &crate::kernel::AggregateSnapshot,
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
fn evaluate_predicate(
    snapshot: &crate::kernel::AggregateSnapshot,
    scope: &crate::kernel::ExecutionId,
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

/// The model children of one container node, borrowed from the declaring
/// model. `Branches` is a branched step's selection, `Catches` a failed
/// step's handlers, `Steps` a branch's own chain.
enum ContainerShape<'a> {
    Branches(&'a [crate::model::Branch]),
    Catches(&'a [crate::model::Step]),
    Steps(&'a [crate::model::Step]),
}

fn walk_step<'a>(step: &'a crate::model::Step, target: &str) -> Option<ContainerShape<'a>> {
    if step.id == target {
        // A step with `catches` is a container over them: the legacy
        // `check_in_children` schedules every child the step declares (the
        // catch steps included), so a success also runs into them — their
        // guards decide what a handler does without an error to answer.
        if !step.catches.is_empty() {
            return Some(ContainerShape::Catches(&step.catches));
        }
        return model::step_branches(step).map(ContainerShape::Branches);
    }
    for branch in model::step_branches(step).unwrap_or_default() {
        if branch.id == target {
            return Some(ContainerShape::Steps(&branch.steps));
        }
        for inner in &branch.steps {
            if let Some(found) = walk_step(inner, target) {
                return Some(found);
            }
        }
    }
    for catch in &step.catches {
        if let Some(found) = walk_step(catch, target) {
            return Some(found);
        }
    }
    // A timeout branch can be a container of its own (a step with branches or
    // catches): the fan-out that runs it looks its shape up here too.
    for timeout in &step.timeouts {
        if let Some(found) = walk_step(timeout, target) {
            return Some(found);
        }
    }
    None
}

fn container_shape<'a>(
    model: &'a crate::model::Workflow,
    model_node: &crate::kernel::NodeId,
) -> Option<ContainerShape<'a>> {
    // The root's children are the seeded chain, never a fan-out.
    if model_node.as_str() == crate::utils::consts::TASK_ROOT_TID {
        return None;
    }
    model
        .steps
        .iter()
        .find_map(|step| walk_step(step, model_node.as_str()))
}

fn node_id(name: &str) -> Result<crate::kernel::NodeId> {
    crate::kernel::NodeId::parse(name).map_err(runtime)
}

/// The step node with this id anywhere in the model (top level and inside
/// branches) — the effect seam's lookup for a `uses` step.
fn find_step_in_model<'a>(
    model: &'a crate::model::Workflow,
    target: &str,
) -> Option<&'a crate::model::Step> {
    model.steps.iter().find_map(|step| find_step(step, target))
}

fn find_step<'a>(step: &'a crate::model::Step, target: &str) -> Option<&'a crate::model::Step> {
    if step.id == target {
        return Some(step);
    }
    for branch in model::step_branches(step).unwrap_or_default() {
        for inner in &branch.steps {
            if let Some(found) = find_step(inner, target) {
                return Some(found);
            }
        }
    }
    for catch in &step.catches {
        if let Some(found) = find_step(catch, target) {
            return Some(found);
        }
    }
    for timeout in &step.timeouts {
        if let Some(found) = find_step(timeout, target) {
            return Some(found);
        }
    }
    None
}

/// Best-effort wake delivery into the host's queue. Losing a wake is safe:
/// the recovery scan re-derives the same pending operations.
#[derive(Clone)]
struct QueueWake {
    tx: Sender<OperationId>,
}

impl QueueWake {
    fn wake(&self, operation: &OperationId) {
        // A full or closed queue loses the wake by design; the recovery scan
        // is the durable net.
        let _ = self.tx.try_send(operation.clone());
    }
}

/// What one quiescence pass did.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RunReport {
    pub(crate) executed: usize,
    pub(crate) retried: usize,
    pub(crate) ignored: usize,
}

/// The engine-facing kernel host: a cloneable handle over the aggregate
/// documents. The wake channel's receiving end is returned by [`KernelHost::new`]
/// and belongs to exactly one driver task ([`KernelHost::serve`]).
#[derive(Clone)]
pub(crate) struct KernelHost {
    aggregates: Arc<AsyncAggregateRepository>,
    recovery: Arc<AsyncRecoveryRepository>,
    wake_tx: Sender<OperationId>,
    /// The effect seam: when attached, a Func package's work runs inside the
    /// ApplyAction hop and its outputs commit with the completion. Without
    /// one the host auto-completes (the shadow contract).
    effect: Arc<parking_lot::RwLock<Option<Arc<dyn effects::EffectRunner>>>>,
    /// The outbox delivery seam: the shadow logs; the driver's transport
    /// maps records onto engine messages (see `message::message_from_record`).
    transport: Arc<parking_lot::RwLock<Box<dyn crate::kernel::OutboxTransport>>>,
    /// The process-lifecycle seam: when attached, a run's start and terminal
    /// are reported as engine events — the driver's mirror of the legacy
    /// `on_proc` emission. The host builds the `Message` from the aggregate,
    /// its only source.
    lifecycle: Arc<parking_lot::RwLock<Option<LifecycleSink>>>,
    /// The read-model seam: when attached, every commit re-materializes the
    /// legacy rows (`procs`/`tasks`/`vars`) from the committed snapshot, so
    /// the row-reading APIs and the sweeper see the run (see `projection`).
    /// Without one the host keeps no read model — the kernel's own tests.
    projection: Arc<parking_lot::RwLock<Option<Arc<projection::RowProjector>>>>,
    /// The predicate seam: when attached, a branch `if` is decided on the
    /// engine's own environment (`ctx.eval`, engine functions included) on a
    /// headless task, exactly like the legacy branch task; without one the
    /// built-in subset evaluator decides (see `predicate`).
    predicate: Arc<parking_lot::RwLock<Option<Arc<dyn predicate::PredicateRunner>>>>,
    /// Which runs were already reported: a start once, a terminal once.
    reported: Arc<parking_lot::Mutex<ReportedRuns>>,
}

/// Which engine event a run's lifecycle report is: the driver's mirror of the
/// legacy `on_proc` emission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProcessEvent {
    Started,
    Completed,
    Errored,
}

/// The process-lifecycle sink: the engine routes it onto its channel events
/// (`on_start`/`on_complete`/`on_error`).
pub(crate) type LifecycleSink = Box<dyn Fn(&crate::Message, ProcessEvent) + Send + Sync>;

/// Which runs were already reported: a start once, a terminal once.
#[derive(Default)]
struct ReportedRuns {
    started: std::collections::HashSet<String>,
    finished: std::collections::HashSet<String>,
}

/// The default outbox transport: it logs each record and confirms it — what a
/// host with nothing attached (the kernel's own tests, and any embedder that
/// only wants the durable aggregate) does. The serving host replaces it with
/// [`message::MessageForwarder`] so records reach the engine channel.
struct ShadowTransport;

impl crate::kernel::OutboxTransport for ShadowTransport {
    fn deliver(&mut self, record: &crate::kernel::OutboxRecord) -> crate::kernel::DeliveryOutcome {
        tracing::debug!(
            pid = %record.id.process_id,
            key = %record.id.key,
            topic = %record.topic,
            "kernel outbox delivered (shadow)"
        );
        crate::kernel::DeliveryOutcome::Delivered {
            receipt: "shadow".into(),
        }
    }
}

impl KernelHost {
    /// A host over the engine's key-value store, plus the wake receiver its
    /// driver task must own. Tests pass `Arc::new(MemoryStore::new())`.
    pub(crate) fn new(kv: Arc<dyn KvStore>) -> (Arc<Self>, Receiver<OperationId>) {
        let store = Arc::new(KvDocumentStore::new(kv));
        let aggregates = Arc::new(AsyncAggregateRepository::new(store));
        let recovery = Arc::new(AsyncRecoveryRepository::new(aggregates.clone()));
        let (wake_tx, wake_rx) = channel(PAGE_SIZE * 8);
        (
            Arc::new(Self {
                aggregates,
                recovery,
                wake_tx,
                effect: Arc::new(parking_lot::RwLock::new(None)),
                transport: Arc::new(parking_lot::RwLock::new(Box::new(ShadowTransport))),
                lifecycle: Arc::new(parking_lot::RwLock::new(None)),
                projection: Arc::new(parking_lot::RwLock::new(None)),
                predicate: Arc::new(parking_lot::RwLock::new(None)),
                reported: Arc::new(parking_lot::Mutex::new(ReportedRuns::default())),
            }),
            wake_rx,
        )
    }

    /// A host over an in-memory store for tests and the P2 parity harness.
    pub(crate) fn in_memory() -> (Arc<Self>, Receiver<OperationId>) {
        Self::new(Arc::new(MemoryStore::new()))
    }

    /// Attach the effect runner: from then on a Func package's work runs
    /// inside the ApplyAction hop (see [`effects`]). Idempotent — the last
    /// attach wins; attach before driving hops.
    pub(crate) fn attach_effect_runner(&self, runner: Arc<dyn effects::EffectRunner>) {
        *self.effect.write() = Some(runner);
    }

    /// Replace the outbox transport: the shadow's logs records; the driver's
    /// transport maps them onto engine messages (see [`message`]). Attach
    /// before driving hops.
    pub(crate) fn attach_outbox_transport(
        &self,
        transport: Box<dyn crate::kernel::OutboxTransport>,
    ) {
        *self.transport.write() = transport;
    }

    /// Attach the process-lifecycle sink: from then on a run reports one
    /// start when it is alive and one terminal event when every execution
    /// settled — the driver's `on_proc`. Attach before driving hops.
    pub(crate) fn attach_lifecycle_sink(&self, sink: LifecycleSink) {
        *self.lifecycle.write() = Some(sink);
    }

    /// Attach the read-model projector: from then on every commit
    /// re-materializes the legacy rows from the committed snapshot (see
    /// [`projection`]). Attach before driving hops.
    pub(crate) fn attach_row_projector(&self, projector: Arc<projection::RowProjector>) {
        *self.projection.write() = Some(projector);
    }

    /// Attach the predicate runner: from then on a branch `if` is decided on
    /// the engine's own environment on a headless task (see [`predicate`]);
    /// without one the built-in subset evaluator decides. Attach before
    /// driving hops.
    pub(crate) fn attach_predicate_runner(&self, runner: Arc<dyn predicate::PredicateRunner>) {
        *self.predicate.write() = Some(runner);
    }

    /// Decide one branch `if`: on the engine's environment when a predicate
    /// runner is attached — a compile/eval failure of a *usable* expression
    /// included — otherwise on the built-in subset evaluator, which knows
    /// variables, operators and value methods but no engine function.
    async fn evaluate_branch_predicate(
        &self,
        snapshot: &crate::kernel::AggregateSnapshot,
        execution_id: &crate::kernel::ExecutionId,
        expr: &str,
        inputs: &[(String, String)],
    ) -> Result<PredicateValue> {
        let runner = self.predicate.read().clone();
        let (Some(runner), Some(model)) = (runner, model_of(snapshot)?) else {
            return Ok(evaluate_predicate(snapshot, execution_id, expr, inputs));
        };
        match runner
            .evaluate(predicate::PredicateRequest {
                scope: self.scope_spec(snapshot, execution_id, model)?,
                expr: expr.to_string(),
                overlay: inputs.to_vec(),
            })
            .await
        {
            Ok(value) => Ok(value),
            // A predicate the engine's environment cannot decide — an unset
            // variable, a non-boolean, a compile error — is the legacy branch
            // task's `init` error: the branch fails, the hop does not.
            Err(error) => Ok(PredicateValue::Invalid {
                message: format!("{error}"),
            }),
        }
    }

    /// Project one committed snapshot into the legacy rows, when a projector
    /// is attached. The rows are a derived read model: a store failure is
    /// reported and dropped, never propagated into the hop — the next commit
    /// re-derives them from the aggregate.
    async fn project(&self, snapshot: &crate::kernel::AggregateSnapshot) {
        let projector = self.projection.read().clone();
        let Some(projector) = projector else {
            return;
        };
        if let Err(error) = projector.project(snapshot).await {
            tracing::warn!(
                pid = %snapshot.process_id,
                error = %error,
                "row projection failed; the next commit re-derives the rows"
            );
        }
    }

    /// Project only the executions one commit touched (plus the proc row): the
    /// read model's rows for the rest of the run cannot have changed.
    async fn project_touched(
        &self,
        snapshot: &crate::kernel::AggregateSnapshot,
        touched: &std::collections::BTreeSet<String>,
    ) {
        let projector = self.projection.read().clone();
        let Some(projector) = projector else {
            return;
        };
        if let Err(error) = projector.project_nodes(snapshot, Some(touched)).await {
            tracing::warn!(
                pid = %snapshot.process_id,
                error = %error,
                "row projection failed; the next commit re-derives the rows"
            );
        }
    }

    /// Re-project one process from its durable aggregate (the action path, and
    /// the outbox settling pass: a run whose last hop committed before its
    /// records were delivered only becomes `removable` once they are).
    pub(crate) async fn refresh_projection(&self, process_id: &str) {
        if self.projection.read().is_none() {
            return;
        }
        match self.load(process_id).await {
            Ok(Some(loaded)) => self.project(&loaded.snapshot).await,
            Ok(None) => {}
            Err(error) => tracing::warn!(
                pid = %process_id,
                error = %error,
                "row projection refresh failed; the next commit re-derives the rows"
            ),
        }
    }

    /// Re-project one process's row only: the deliveries just settled, so the
    /// sole fact that can have changed is the sweeper's `removable` flag.
    async fn refresh_proc_row(&self, process_id: &str) {
        let projector = self.projection.read().clone();
        let Some(projector) = projector else {
            return;
        };
        match self.load(process_id).await {
            Ok(Some(loaded)) => {
                if let Err(error) = projector.project_proc_row(&loaded.snapshot).await {
                    tracing::warn!(
                        pid = %process_id,
                        error = %error,
                        "proc row projection failed; the next commit re-derives it"
                    );
                }
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(
                pid = %process_id,
                error = %error,
                "proc row projection refresh failed"
            ),
        }
    }

    /// Report one run's lifecycle from a fresh snapshot, at most once per
    /// phase. Mirrors the legacy `on_proc`: a start while the run is alive
    /// (running or pending); otherwise — every execution terminal — an error
    /// for a failed root, a completion for any other terminal, with the
    /// model's `exposes` validated against the run's outputs (a validation
    /// failure is reported as an error, exactly like the legacy handler).
    fn report_lifecycle(&self, snapshot: &crate::kernel::AggregateSnapshot) {
        let lifecycle = self.lifecycle.read();
        let Some(sink) = lifecycle.as_ref() else {
            return;
        };
        let Some(root) = snapshot
            .executions
            .keys()
            .filter(|id| id.task_id == crate::utils::consts::TASK_ROOT_TID)
            .min_by_key(|id| id.generation)
            .cloned()
        else {
            return;
        };
        let root_state = snapshot
            .executions
            .get(&root)
            .map(|execution| execution.state);
        let all_terminal = !snapshot.executions.is_empty()
            && snapshot
                .executions
                .values()
                .all(|execution| execution.state.is_terminal());
        let mut reported = self.reported.lock();
        if all_terminal {
            if !reported.finished.insert(snapshot.process_id.clone()) {
                return;
            }
            let mut message = process_message(snapshot, &root);
            if root_state == Some(crate::kernel::ExecutionState::Failed) {
                let (code, text) = root_error(snapshot, &root);
                message.set_err(&code, &text);
                sink(&message, ProcessEvent::Errored);
                return;
            }
            match exposes_error(snapshot, &message) {
                Some(error) => {
                    message.set_err("", &error);
                    sink(&message, ProcessEvent::Errored);
                }
                None => sink(&message, ProcessEvent::Completed),
            }
            return;
        }
        if reported.started.insert(snapshot.process_id.clone()) {
            let message = process_message(snapshot, &root);
            sink(&message, ProcessEvent::Started);
        }
    }

    /// The background driver: executes woken hops as they arrive, and on every
    /// tick re-derives recovery work (the durable net) and runs one outbox
    /// delivery pass. Exits when `shutdown` fires.
    pub(crate) async fn serve(
        self: Arc<Self>,
        mut wake_rx: Receiver<OperationId>,
        shutdown: tokio_util::sync::CancellationToken,
        tick: std::time::Duration,
    ) {
        let mut ticker = tokio::time::interval(tick);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = ticker.tick() => {
                    let now = SystemClock.now_ms();
                    if let Err(error) = self.recover(now).await {
                        tracing::warn!(error = %error, "kernel recovery scan failed");
                    }
                    if let Err(error) = self.dispatch_outbox().await {
                        tracing::warn!(error = %error, "kernel outbox pass failed");
                    }
                }
                id = wake_rx.recv() => {
                    let Some(id) = id else { break };
                    if let Err(error) = self.execute_one(&id).await {
                        // The operation stays open; the next recovery tick
                        // re-derives it.
                        #[cfg(test)]
                        eprintln!("hop {id:?} failed: {error}");
                        tracing::debug!(operation = ?id, error = %error, "kernel hop failed; left open for recovery");
                    }
                }
            }
        }
    }

    /// Create the process atomically and wake its root operation.
    pub(crate) async fn start_process(&self, spec: &ProcessSpec) -> Result<String> {
        let snapshot = spec.seed_snapshot()?;
        self.aggregates.create(&snapshot).await.map_err(runtime)?;
        // The read model from the first durable fact: the proc row and the
        // seeded task rows exist before the root hop runs, so an immediately
        // following read sees the run.
        self.project(&snapshot).await;
        let root = OperationId {
            process_id: spec.process_id.clone(),
            task_id: spec.root.clone(),
            generation: 0,
            sequence: 0,
        };
        QueueWake {
            tx: self.wake_tx.clone(),
        }
        .wake(&root);
        Ok(spec.process_id.clone())
    }

    /// Drive woken operations until the queue drains or `max_steps` hops ran.
    /// Test-side twin of [`Self::serve`]: same hop execution, deterministic
    /// drain instead of a select loop.
    pub(crate) async fn run_until_quiescent(
        &self,
        wake_rx: &mut Receiver<OperationId>,
        max_steps: usize,
    ) -> Result<RunReport> {
        let mut report = RunReport::default();
        for _ in 0..max_steps {
            let id = match wake_rx.try_recv() {
                Ok(id) => id,
                Err(_) => break,
            };
            match self.execute_one(&id).await {
                Ok(true) => report.executed += 1,
                Ok(false) => report.ignored += 1,
                Err(error) => {
                    report.retried += 1;
                    #[cfg(test)]
                    eprintln!("hop {id:?} failed: {error}");
                    tracing::debug!(operation = ?id, error = %error, "hop failed; operation stays open for recovery");
                }
            }
        }
        Ok(report)
    }

    /// Apply a client action to one execution: the durable intent (an
    /// `ApplyAction` operation) is created first, then owned, re-planned
    /// against the authoritative snapshot, committed, and completed — the
    /// same sequence [`crate::kernel::ActionApplicationService`] uses. A
    /// semantically invalid action is rejected before anything durable
    /// happens.
    pub(crate) async fn apply_action(
        &self,
        process_id: &str,
        task_id: &str,
        action: crate::kernel::ActionKind,
        inputs: Vec<(String, String)>,
    ) -> Result<()> {
        // The serving loop commits hops of the same aggregate concurrently, so
        // a client's plan can go stale before its commit. A retry re-plans
        // against the fresh snapshot: the action either applies or is
        // legitimately refused, and a refusal is never retried.
        let mut attempt = 0;
        loop {
            match self
                .apply_action_once(process_id, task_id, action.clone(), inputs.clone())
                .await
            {
                Ok(()) => return Ok(()),
                Err(error) => {
                    if !is_conflict(&error) || attempt + 1 >= ACTION_ATTEMPTS {
                        return Err(error);
                    }
                    tracing::debug!(
                        pid = %process_id,
                        tid = %task_id,
                        attempt,
                        "action lost a compare-and-swap; re-planning"
                    );
                    attempt += 1;
                }
            }
        }
    }

    /// One action application: the durable intent, ownership, the re-plan
    /// against the authoritative snapshot, the commit and the completion (see
    /// [`Self::apply_action`] for the retry around it).
    async fn apply_action_once(
        &self,
        process_id: &str,
        task_id: &str,
        action: crate::kernel::ActionKind,
        inputs: Vec<(String, String)>,
    ) -> Result<()> {
        let snapshot = self
            .load(process_id)
            .await?
            .ok_or_else(|| ActError::Runtime(format!("aggregate {process_id} not found")))?
            .snapshot;
        let execution_id = snapshot
            .executions
            .keys()
            .filter(|id| id.task_id == task_id)
            .max_by_key(|id| id.generation)
            .cloned()
            .ok_or_else(|| ActError::Runtime(format!("execution {task_id} not found")))?;
        let next_sequence = snapshot
            .operations
            .keys()
            .filter(|id| {
                id.process_id == process_id
                    && id.task_id == task_id
                    && id.generation == execution_id.generation
            })
            .map(|id| id.sequence)
            .max()
            .unwrap_or(0)
            + 1;
        let operation_id = OperationId {
            process_id: process_id.into(),
            task_id: task_id.into(),
            generation: execution_id.generation,
            sequence: next_sequence,
        };
        let now = SystemClock.now_ms();
        let claim = crate::kernel::ClaimToken {
            owner: OWNER.into(),
            fence: 1,
            lease_until_ms: now + LEASE_MS,
            heartbeat_revision: 0,
        };
        let command = crate::kernel::ApplyActionCommand {
            process_id: process_id.into(),
            execution: execution_id.clone(),
            action,
            inputs,
            source_operation: OperationCommit {
                id: operation_id.clone(),
                claim: claim.clone(),
            },
        };

        // Preflight against the current snapshot: a caller-correctable
        // rejection (terminal task, unknown execution) consumes nothing.
        crate::kernel::ActionPlanner::plan(&snapshot, &command).map_err(runtime)?;
        // A step declaring `catches` is not a client target once it is
        // deciding — or has dispatched — its caught error: the legacy act it
        // folds is already terminal, and the legacy `do_action` refuses every
        // action on it. The refusal is part of the preflight, so it consumes
        // no operation.
        if let Some(model) = model_of(&snapshot)?
            && let Some(node) = snapshot
                .executions
                .get(&execution_id)
                .map(|execution| execution.model_node.as_str())
            && let Some(step) = find_step_in_model(&model, node)
            && !step.catches.is_empty()
            && (is_catch_decider(&snapshot, &execution_id)
                || snapshot
                    .executions
                    .values()
                    .any(|child| child.parent.as_ref() == Some(&execution_id)))
        {
            return Err(ActError::Action(format!(
                "step '{}' is deciding (or already dispatched) its catches; the act it folds is decided",
                execution_id.task_id
            )));
        }

        // Durable intent.
        let intent = TransitionPlan {
            process_id: process_id.into(),
            expected: snapshot.version.clone(),
            execution_creates: Vec::new(),
            state_changes: Vec::new(),
            variable_writes: Vec::new(),
            source_operation: None,
            operations: vec![OperationIntent {
                id: operation_id.clone(),
                kind: OperationKind::ApplyAction,
            }],
            outbox: Vec::new(),
        };
        match self
            .aggregates
            .commit_transition(&intent)
            .await
            .map_err(repository_error)?
        {
            crate::kernel::TransitionCommitOutcome::Committed { .. }
            | crate::kernel::TransitionCommitOutcome::AlreadyCommitted { .. } => {}
            crate::kernel::TransitionCommitOutcome::Conflict { .. } => {
                return Err(conflict(format!(
                    "action intent conflict on {process_id}/{task_id}"
                )));
            }
        }

        // Ownership, authoritative re-plan, commit, complete — the standard
        // hop, but planned by ActionPlanner instead of the host's own arms.
        let revision = self
            .load(process_id)
            .await?
            .ok_or_else(|| ActError::Runtime("aggregate lost during action".into()))?
            .snapshot
            .operations
            .get(&operation_id)
            .map(|record| record.revision)
            .ok_or_else(|| ActError::Runtime("action operation lost".into()))?;
        let claimed = self
            .operation_event(
                &operation_id,
                revision,
                OperationEvent::Claim(claim.clone()),
            )
            .await?;
        let prepared = self
            .operation_event(
                &operation_id,
                claimed.revision,
                OperationEvent::PrepareCommit {
                    claim: claim.clone(),
                },
            )
            .await?;
        let snapshot = self
            .load(process_id)
            .await?
            .ok_or_else(|| ActError::Runtime("aggregate lost during action".into()))?
            .snapshot;
        // The authoritative effect: a container completion fans out instead
        // of completing (see `effect_plan`); every other action is the
        // ActionPlanner's own plan.
        let effect = self
            .effect_plan(
                &snapshot,
                &operation_id,
                &execution_id,
                snapshot.executions.get(&execution_id).ok_or_else(|| {
                    ActError::Runtime(format!("execution {execution_id:?} lost during action"))
                })?,
                &command.source_operation,
                Some(command.action.clone()),
                command.inputs.clone(),
            )
            .await;
        match effect {
            Ok(plan) => {
                let receipt = match self
                    .aggregates
                    .commit_transition(&plan)
                    .await
                    .map_err(repository_error)?
                {
                    crate::kernel::TransitionCommitOutcome::Committed { receipt, .. }
                    | crate::kernel::TransitionCommitOutcome::AlreadyCommitted {
                        receipt, ..
                    } => receipt,
                    crate::kernel::TransitionCommitOutcome::Conflict { .. } => {
                        return Err(conflict(format!(
                            "action commit conflict on {process_id}/{task_id}"
                        )));
                    }
                };
                let committed = self
                    .load(process_id)
                    .await?
                    .ok_or_else(|| ActError::Runtime("aggregate lost after action".into()))?
                    .snapshot
                    .operations
                    .get(&operation_id)
                    .cloned()
                    .ok_or_else(|| ActError::Runtime("action operation lost".into()))?;
                self.operation_event(
                    &operation_id,
                    committed.revision,
                    OperationEvent::Complete {
                        commit: receipt.token.clone(),
                    },
                )
                .await?;
                let wake = QueueWake {
                    tx: self.wake_tx.clone(),
                };
                for operation in &receipt.new_operations {
                    wake.wake(operation);
                }
                // A Cancel ends this path AND re-runs it: the cancelled
                // execution is re-entered with a fresh generation (the
                // legacy rewind), its downstream recreated by the planner.
                // A Back re-enters the step the client named (`to`): the
                // backed execution stays terminal; the named step and its
                // downstream get fresh generations.
                if matches!(command.action, crate::kernel::ActionKind::Cancel) {
                    self.run_reentry(process_id, &execution_id, task_id).await?;
                } else if matches!(command.action, crate::kernel::ActionKind::Back) {
                    let to = command
                        .inputs
                        .iter()
                        .find(|(name, _)| name == crate::utils::consts::ACT_TO)
                        .map(|(_, value)| value.clone())
                        .ok_or_else(|| {
                            ActError::Runtime("cannot find 'to' value in options".to_string())
                        })?;
                    self.run_reentry(process_id, &execution_id, &to).await?;
                }
                // The read model follows the action's commits (the re-entry
                // included).
                self.refresh_projection(process_id).await;
                Ok(())
            }
            Err(error) => {
                // The state moved between intent and ownership: park the
                // operation for recovery instead of consuming it.
                let _ = self
                    .operation_event(
                        &operation_id,
                        prepared.revision,
                        OperationEvent::ScheduleRetry {
                            claim: claim.clone(),
                            attempt: 1,
                        },
                    )
                    .await;
                Err(ActError::Runtime(format!("action re-plan failed: {error}")))
            }
        }
    }

    /// Re-enter one cancelled execution with a fresh generation through
    /// [`crate::kernel::GenerationPlanner`]: a carrier operation on the same
    /// execution is created, owned, and committed with the planner's re-entry
    /// plan (new generations for the forward closure).
    ///
    /// The retry is *inside* this method, not around the action: the re-entry
    /// is the substance of a cancel/back, so a lost compare-and-swap must not
    /// leave the source terminal with no successor. Each attempt recomputes
    /// the carrier against the fresh snapshot — a stranded carrier from a
    /// failed attempt is inert (its execution is already decided, so the hop
    /// commits the empty remainder).
    async fn run_reentry(
        &self,
        process_id: &str,
        execution_id: &crate::kernel::ExecutionId,
        task_id: &str,
    ) -> Result<()> {
        let mut attempt = 0;
        loop {
            match self
                .run_reentry_once(process_id, execution_id, task_id)
                .await
            {
                Ok(()) => return Ok(()),
                Err(error) => {
                    if !is_conflict(&error) || attempt + 1 >= ACTION_ATTEMPTS {
                        return Err(error);
                    }
                    tracing::debug!(
                        pid = %process_id,
                        task = %execution_id.task_id,
                        attempt,
                        "reentry lost a compare-and-swap; re-planning"
                    );
                    attempt += 1;
                }
            }
        }
    }

    /// One re-entry attempt (see [`Self::run_reentry`]).
    async fn run_reentry_once(
        &self,
        process_id: &str,
        execution_id: &crate::kernel::ExecutionId,
        task_id: &str,
    ) -> Result<()> {
        let snapshot = self
            .load(process_id)
            .await?
            .ok_or_else(|| ActError::Runtime("aggregate lost before reentry".into()))?
            .snapshot;
        let next_sequence = snapshot
            .operations
            .keys()
            .filter(|id| {
                id.process_id == process_id
                    && id.task_id == execution_id.task_id
                    && id.generation == execution_id.generation
            })
            .map(|id| id.sequence)
            .max()
            .unwrap_or(0)
            + 1;
        let carrier_id = OperationId {
            process_id: process_id.into(),
            task_id: execution_id.task_id.clone(),
            generation: execution_id.generation,
            sequence: next_sequence,
        };
        let intent = TransitionPlan {
            process_id: process_id.into(),
            expected: snapshot.version.clone(),
            execution_creates: Vec::new(),
            state_changes: Vec::new(),
            variable_writes: Vec::new(),
            source_operation: None,
            operations: vec![OperationIntent {
                id: carrier_id.clone(),
                kind: OperationKind::ApplyAction,
            }],
            outbox: Vec::new(),
        };
        match self
            .aggregates
            .commit_transition(&intent)
            .await
            .map_err(repository_error)?
        {
            crate::kernel::TransitionCommitOutcome::Committed { .. }
            | crate::kernel::TransitionCommitOutcome::AlreadyCommitted { .. } => {}
            crate::kernel::TransitionCommitOutcome::Conflict { .. } => {
                return Err(conflict("reentry intent conflict"));
            }
        }
        let now = SystemClock.now_ms();
        let claim = crate::kernel::ClaimToken {
            owner: OWNER.into(),
            fence: 1,
            lease_until_ms: now + LEASE_MS,
            heartbeat_revision: 0,
        };
        let revision = self
            .load(process_id)
            .await?
            .ok_or_else(|| ActError::Runtime("aggregate lost during reentry".into()))?
            .snapshot
            .operations
            .get(&carrier_id)
            .map(|record| record.revision)
            .ok_or_else(|| ActError::Runtime("reentry carrier lost".into()))?;
        let claimed = self
            .operation_event(&carrier_id, revision, OperationEvent::Claim(claim.clone()))
            .await?;
        let prepared = self
            .operation_event(
                &carrier_id,
                claimed.revision,
                OperationEvent::PrepareCommit {
                    claim: claim.clone(),
                },
            )
            .await?;
        let snapshot = self
            .load(process_id)
            .await?
            .ok_or_else(|| ActError::Runtime("aggregate lost during reentry".into()))?
            .snapshot;
        let graph = execution_graph(&snapshot)?;
        let command = crate::kernel::ReenterCommand {
            process_id: process_id.into(),
            source: execution_id.clone(),
            target_task_id: task_id.into(),
            source_operation: OperationCommit {
                id: carrier_id.clone(),
                claim: claim.clone(),
            },
        };
        let plan = crate::kernel::GenerationPlanner::plan_reentry(&graph, &snapshot, &command)
            .map_err(runtime)?;
        let receipt = match self
            .aggregates
            .commit_transition(&plan)
            .await
            .map_err(repository_error)?
        {
            crate::kernel::TransitionCommitOutcome::Committed { receipt, .. }
            | crate::kernel::TransitionCommitOutcome::AlreadyCommitted { receipt, .. } => receipt,
            crate::kernel::TransitionCommitOutcome::Conflict { .. } => {
                return Err(conflict("reentry commit conflict"));
            }
        };
        let committed = self
            .load(process_id)
            .await?
            .ok_or_else(|| ActError::Runtime("aggregate lost after reentry".into()))?
            .snapshot
            .operations
            .get(&carrier_id)
            .cloned()
            .ok_or_else(|| ActError::Runtime("carrier lost after reentry".into()))?;
        let _ = prepared;
        self.operation_event(
            &carrier_id,
            committed.revision,
            OperationEvent::Complete {
                commit: receipt.token.clone(),
            },
        )
        .await?;
        let wake = QueueWake {
            tx: self.wake_tx.clone(),
        };
        for operation in &receipt.new_operations {
            wake.wake(operation);
        }
        Ok(())
    }

    /// One durable hop. Returns `Ok(false)` when the operation needed nothing
    /// (a wake redelivered for an already-terminal operation).
    async fn execute_one(&self, id: &OperationId) -> Result<bool> {
        let now = SystemClock.now_ms();
        let snapshot = self
            .load(&id.process_id)
            .await?
            .ok_or_else(|| ActError::Runtime(format!("aggregate {} not found", id.process_id)))?
            .snapshot;
        let record = snapshot
            .operations
            .get(id)
            .cloned()
            .ok_or_else(|| ActError::Runtime(format!("operation {id:?} not found")))?;
        match record.state {
            OperationState::Pending | OperationState::RetryScheduled { .. } => {}
            _ => return Ok(false),
        }

        // The timeout timer wakes once per tick and usually has nothing to do:
        // evaluating its branches before the claim keeps an idle round from
        // consuming the operation (it stays `Pending`, so the next recovery
        // scan re-wakes it — the kernel's tick cadence) and from writing
        // anything. Only the *idle* verdict short-circuits here; the round is
        // decided again after the claim, against the authoritative snapshot.
        if record.kind == OperationKind::DispatchTimeout {
            let execution_id = crate::kernel::ExecutionId {
                task_id: id.task_id.clone(),
                generation: id.generation,
            };
            let execution = snapshot.executions.get(&execution_id);
            if matches!(
                self.timeout_round(&snapshot, &execution_id, execution, now)
                    .await?,
                TimeoutRound::Idle
            ) {
                return Ok(false);
            }
        }

        let claim = crate::kernel::ClaimToken {
            owner: OWNER.into(),
            fence: 1,
            lease_until_ms: now + LEASE_MS,
            heartbeat_revision: 0,
        };
        let claimed = self
            .operation_event(id, record.revision, OperationEvent::Claim(claim.clone()))
            .await?;
        let prepared = self
            .operation_event(
                id,
                claimed.revision,
                OperationEvent::PrepareCommit {
                    claim: claim.clone(),
                },
            )
            .await?;
        debug_assert!(matches!(prepared.state, OperationState::CommitPrepared(_)));

        // Re-read after prepare: ownership moved the aggregate version, so the
        // plan must be formed from the authoritative snapshot.
        let snapshot = self
            .load(&id.process_id)
            .await?
            .ok_or_else(|| ActError::Runtime(format!("aggregate {} not found", id.process_id)))?
            .snapshot;
        let execution_id = crate::kernel::ExecutionId {
            task_id: id.task_id.clone(),
            generation: id.generation,
        };
        let execution = snapshot.executions.get(&execution_id).cloned();
        let commit_claim = OperationCommit {
            id: id.clone(),
            claim: crate::kernel::ClaimToken {
                owner: claim.owner.clone(),
                fence: claim.fence,
                lease_until_ms: claim.lease_until_ms,
                heartbeat_revision: claim.heartbeat_revision,
            },
        };

        let plan = match record.kind {
            OperationKind::ExecuteTask => {
                let action_id = OperationId {
                    process_id: id.process_id.clone(),
                    task_id: id.task_id.clone(),
                    generation: id.generation,
                    sequence: id.sequence + 1,
                };
                let action_exists = snapshot.operations.contains_key(&action_id);
                // Auto tasks seed their own completion action; waiting tasks
                // stay Running until a client action arrives.
                let auto = snapshot
                    .variables
                    .get(&(execution_id.clone(), "auto".into()))
                    .map(String::as_str)
                    != Some("0");
                self.execute_task_plan(
                    &snapshot,
                    id,
                    &execution_id,
                    execution.as_ref(),
                    &commit_claim,
                    &action_id,
                    action_exists,
                    auto,
                    now,
                )
                .await?
            }
            OperationKind::ApplyAction => {
                let execution = execution.ok_or_else(|| {
                    ActError::Runtime(format!("execution {execution_id:?} not found"))
                })?;
                if !matches!(
                    execution.state,
                    crate::kernel::ExecutionState::Running | crate::kernel::ExecutionState::Waiting
                ) {
                    // A decision landed while this hop was in flight (the
                    // cascade skipped the execution, a cancel closed it): the
                    // durable remainder is the empty plan, not an error.
                    self.task_hop_plan(
                        &snapshot,
                        id,
                        &execution_id,
                        Some(&execution),
                        &commit_claim,
                        "completed",
                        "task.completed",
                    )?
                } else {
                    // A container fans out instead of completing; a plain task
                    // completes and propagates.
                    self.effect_plan(
                        &snapshot,
                        id,
                        &execution_id,
                        &execution,
                        &commit_claim,
                        None,
                        Vec::new(),
                    )
                    .await?
                }
            }
            OperationKind::SkipTask => {
                // The cascade hop: admit-and-skip already committed
                // `Created → Ready`; this hop marks the execution `Skipped`
                // and hands propagation to a `PropagateTask`, whose readiness
                // treats the skipped source as dead and continues the
                // cascade. Like every hop it is one atomic commit sourced by
                // this operation.
                let execution = execution.ok_or_else(|| {
                    ActError::Runtime(format!("execution {execution_id:?} not found"))
                })?;
                if !matches!(
                    execution.state,
                    crate::kernel::ExecutionState::Ready
                        | crate::kernel::ExecutionState::Running
                        | crate::kernel::ExecutionState::Waiting
                ) {
                    return Err(ActError::Runtime(format!(
                        "execution {execution_id:?} is {:?}, expected Ready, Running or Waiting",
                        execution.state
                    )));
                }
                let propagate_id = OperationId {
                    process_id: id.process_id.clone(),
                    task_id: id.task_id.clone(),
                    generation: id.generation,
                    sequence: id.sequence + 1,
                };
                let mut plan = self.task_hop_plan(
                    &snapshot,
                    id,
                    &execution_id,
                    Some(&execution),
                    &commit_claim,
                    "skipped",
                    "task.skipped",
                )?;
                // A skipped act emits its Skipped message (the legacy act
                // row's own message).
                if let Some(payload) = self.act_message_payload(
                    &snapshot,
                    &execution_id,
                    &id.task_id,
                    act_facts(&snapshot, &execution_id).as_ref(),
                    None,
                    None,
                )? {
                    plan.outbox[0].payload = payload.to_string();
                }
                plan.state_changes.push(StateChange {
                    execution: execution_id.clone(),
                    expected_revision: execution.revision,
                    from: execution.state,
                    to: crate::kernel::ExecutionState::Skipped,
                });
                if !snapshot.operations.contains_key(&propagate_id) {
                    plan.operations.push(OperationIntent {
                        id: propagate_id,
                        kind: OperationKind::PropagateTask,
                    });
                }
                plan
            }
            OperationKind::AbortTask => {
                // The abort cascade's running-successor hop: the legacy
                // ancestor walk decides running children `Aborted` (pending
                // ones skip). Same shape as `SkipTask`, terminal `Aborted`.
                let execution = execution.ok_or_else(|| {
                    ActError::Runtime(format!("execution {execution_id:?} not found"))
                })?;
                if execution.state != crate::kernel::ExecutionState::Running {
                    // A decision landed while the hop was queued: the empty
                    // remainder completes the operation either way.
                    self.task_hop_plan(
                        &snapshot,
                        id,
                        &execution_id,
                        Some(&execution),
                        &commit_claim,
                        "aborted",
                        "task.aborted",
                    )?
                } else {
                    let propagate_id = OperationId {
                        process_id: id.process_id.clone(),
                        task_id: id.task_id.clone(),
                        generation: id.generation,
                        sequence: id.sequence + 1,
                    };
                    let mut plan = self.task_hop_plan(
                        &snapshot,
                        id,
                        &execution_id,
                        Some(&execution),
                        &commit_claim,
                        "aborted",
                        "task.aborted",
                    )?;
                    plan.state_changes.push(StateChange {
                        execution: execution_id.clone(),
                        expected_revision: execution.revision,
                        from: crate::kernel::ExecutionState::Running,
                        to: crate::kernel::ExecutionState::Aborted,
                    });
                    if !snapshot.operations.contains_key(&propagate_id) {
                        plan.operations.push(OperationIntent {
                            id: propagate_id,
                            kind: OperationKind::PropagateTask,
                        });
                    }
                    plan
                }
            }
            OperationKind::DispatchTimeout => {
                // The timer of a timed-out execution: the round was already
                // decided before the claim (an idle round never gets here), so
                // this hop either dispatches the due branch, retires a timer
                // with nothing left to fire, or — when a decision landed
                // between the preflight and the claim — re-arms itself for the
                // next tick instead of consuming its own operation.
                match self
                    .timeout_round(&snapshot, &execution_id, execution.as_ref(), now)
                    .await?
                {
                    TimeoutRound::Dispatch {
                        branch,
                        guard,
                        cost_ms,
                        declared,
                    } => self.timeout_plan(
                        &snapshot,
                        id,
                        &execution_id,
                        execution.as_ref(),
                        &commit_claim,
                        branch,
                        guard,
                        cost_ms,
                        declared,
                    )?,
                    // Nothing can fire any more: the owner is decided or its
                    // node declares no timeouts. The operation's own commit is
                    // its whole remainder.
                    TimeoutRound::Retire => Self::empty_remainder(&snapshot, id, &commit_claim),
                    TimeoutRound::Idle => {
                        let _ = self
                            .operation_event(
                                id,
                                prepared.revision,
                                OperationEvent::ScheduleRetry {
                                    claim: claim.clone(),
                                    attempt: 1,
                                },
                            )
                            .await;
                        return Ok(false);
                    }
                }
            }
            OperationKind::PropagateTask => {
                // The graph of record is the snapshot itself (parent map plus
                // predecessor lists), never the declaring spec.
                let graph = execution_graph(&snapshot)?;
                let command = PropagateCommand {
                    process_id: id.process_id.clone(),
                    execution: execution_id.clone(),
                    source_operation: commit_claim,
                    join_policy: crate::kernel::JoinPolicy::AllSucceeded,
                };
                let mut plan =
                    PropagationPlanner::plan(&graph, &snapshot, &command).map_err(runtime)?;
                // The decision walks' upward half: an aborted execution ends
                // its whole ancestor chain `Aborted` (legacy `abort_task`),
                // a failed one ends it `Failed` (legacy `emit_error` →
                // `parent.set_err`); each newly decided ancestor's own hop
                // carries the death into its remaining subtrees.
                let upward_to = match execution.as_ref().map(|e| e.state) {
                    Some(crate::kernel::ExecutionState::Aborted) => {
                        Some(crate::kernel::ExecutionState::Aborted)
                    }
                    Some(crate::kernel::ExecutionState::Failed) => {
                        Some(crate::kernel::ExecutionState::Failed)
                    }
                    _ => None,
                };
                // The container join: a parent parked by a fan-out completes
                // in the same atomic commit that settles its last child; a
                // failed step's CATCH child runs the catch join instead, which
                // either catches the error or resumes it. A decision walk
                // already decides the parent itself (legacy calls
                // `parent.set_err` before the auto-complete pass sees the
                // children settled), so both yield to it — the same change
                // twice is not a plan.
                if upward_to.is_none()
                    && let Some(parent) = execution.as_ref().and_then(|e| e.parent.clone())
                {
                    let catch_join = crate::kernel::ContainerPlanner::plan_catch_join(
                        &graph,
                        &snapshot,
                        &crate::kernel::CatchJoinCommand {
                            process_id: id.process_id.clone(),
                            parent: parent.clone(),
                            source: execution_id.clone(),
                        },
                    )
                    .map_err(runtime)?;
                    match catch_join {
                        Some(crate::kernel::CatchJoinOutcome::Resumed(change, operation)) => {
                            // The record is keyed by the operation that decides
                            // the step — the durable-fact identity the commit
                            // requires (`failed:` keys never repeat).
                            let record_key = format!(
                                "failed:{}:{}:{}:{}",
                                snapshot.process_id,
                                parent.task_id,
                                parent.generation,
                                operation.id.sequence
                            );
                            plan.state_changes.push(change);
                            plan.operations.push(operation);
                            // The step's own failure message: the legacy walk
                            // re-derived the error and emitted the step's task
                            // event (`emit_error`), which is what tells the
                            // client the handlers caught nothing.
                            let code = execution_variable(&snapshot, &parent, "error.code")
                                .map(json_text)
                                .unwrap_or_default();
                            let text = execution_variable(&snapshot, &parent, "error.message")
                                .map(json_text)
                                .unwrap_or_default();
                            if let Some(payload) = self.act_message_payload(
                                &snapshot,
                                &parent,
                                &parent.task_id,
                                act_facts(&snapshot, &parent).as_ref(),
                                None,
                                Some((&code, &text)),
                            )? {
                                plan.outbox.push(crate::kernel::OutboxIntent {
                                    key: record_key,
                                    topic: "task.failed".into(),
                                    payload: payload.to_string(),
                                });
                            }
                        }
                        Some(crate::kernel::CatchJoinOutcome::Caught(change, operation)) => {
                            plan.state_changes.push(change);
                            plan.operations.push(operation);
                        }
                        None => {
                            let join = crate::kernel::ContainerJoinCommand {
                                process_id: id.process_id.clone(),
                                parent,
                            };
                            if let Some((change, operation)) =
                                crate::kernel::ContainerPlanner::plan_join(&graph, &snapshot, &join)
                                    .map_err(runtime)?
                            {
                                plan.state_changes.push(change);
                                plan.operations.push(operation);
                            }
                        }
                    }
                }
                if let Some(to) = upward_to {
                    let (changes, operations) = crate::kernel::UpwardPlanner::plan_upward(
                        &snapshot,
                        &crate::kernel::UpwardCommand {
                            process_id: id.process_id.clone(),
                            source: execution_id.clone(),
                            to,
                        },
                    )
                    .map_err(runtime)?;
                    plan.state_changes.extend(changes);
                    plan.operations.extend(operations);
                }
                plan.validate().map_err(runtime)?;
                plan
            }
            other => {
                return Err(ActError::Runtime(format!(
                    "operation kind {other:?} is not driven by the host yet"
                )));
            }
        };

        let receipt = match self
            .aggregates
            .commit_transition(&plan)
            .await
            .map_err(repository_error)?
        {
            crate::kernel::TransitionCommitOutcome::Committed { receipt, .. }
            | crate::kernel::TransitionCommitOutcome::AlreadyCommitted { receipt, .. } => receipt,
            crate::kernel::TransitionCommitOutcome::Conflict { .. } => {
                // Another writer advanced the aggregate; the wake stays open
                // and recovery re-derives the hop against the new state.
                return Err(ActError::Runtime(format!(
                    "commit conflict on operation {id:?}"
                )));
            }
        };
        let loaded = self
            .load(&id.process_id)
            .await?
            .ok_or_else(|| ActError::Runtime("aggregate lost after commit".into()))?;
        let committed = loaded
            .snapshot
            .operations
            .get(id)
            .cloned()
            .ok_or_else(|| ActError::Runtime("operation lost after commit".into()))?;
        self.operation_event(
            id,
            committed.revision,
            OperationEvent::Complete {
                commit: receipt.token.clone(),
            },
        )
        .await?;
        let wake = QueueWake {
            tx: self.wake_tx.clone(),
        };
        for operation in &receipt.new_operations {
            wake.wake(operation);
        }
        // The run's lifecycle, from the freshly committed state: a start while
        // it is alive, one terminal event once every execution settled. A
        // closed sink or an absent one makes this a no-op.
        self.report_lifecycle(&loaded.snapshot);
        // The read model follows the same commit — only the executions the
        // plan touched (their rows are the only ones that can have changed).
        let touched: std::collections::BTreeSet<String> = plan
            .state_changes
            .iter()
            .map(|change| change.execution.task_id.clone())
            .chain(
                plan.execution_creates
                    .iter()
                    .map(|create| create.execution.task_id.clone()),
            )
            .chain(
                plan.variable_writes
                    .iter()
                    .map(|write| write.scope.task_id.clone()),
            )
            .collect();
        self.project_touched(&loaded.snapshot, &touched).await;
        Ok(true)
    }

    /// The `ExecuteTask` hop: `Ready → Running` (auto) or `Ready → Waiting` (a
    /// waiting package), the act's start-time package resolution — the legacy
    /// act's `run()` validates params before it either waits or executes — the
    /// `Created` message record and, for an auto step, the `ApplyAction`
    /// operation. A resolution failure returns the failure plan instead (the
    /// execution decided `Failed`, the error walk taking over).
    #[allow(clippy::too_many_arguments)]
    async fn execute_task_plan(
        &self,
        snapshot: &crate::kernel::AggregateSnapshot,
        id: &OperationId,
        execution_id: &crate::kernel::ExecutionId,
        execution: Option<&crate::kernel::ExecutionSnapshot>,
        claim: &OperationCommit,
        action_id: &OperationId,
        action_exists: bool,
        auto: bool,
        now_ms: i64,
    ) -> Result<TransitionPlan> {
        let mut plan = self.task_hop_plan(
            snapshot,
            id,
            execution_id,
            execution,
            claim,
            "started",
            "task.started",
        )?;
        // A branch whose predicate could not be evaluated fails on its first
        // hop: the fan-out admitted it with the failure pinned (see
        // `fanout_command`), which is the legacy branch task erroring in its
        // `init` — the container's error walk takes over from here.
        if let Some(execution) = execution
            && let Some(failure) = branch_error(snapshot, execution_id)
        {
            return self
                .failure_plan(snapshot, id, execution_id, execution, claim, &failure)
                .await;
        }
        // The act's package facts and its own gate: every `uses` step resolves
        // its package at start, and the pinned facts ride every later message
        // record — so no later hop re-runs the headless scope, whose snapshot
        // seal must fail the act at start, never a completion.
        let created_key = format!(
            "message.created:{}:{}:{}",
            id.process_id, id.task_id, id.generation
        );
        let created_exists = snapshot
            .outbox
            .keys()
            .any(|outbox_id| outbox_id.key == created_key);
        // Whether the acting node is a `uses` step at all: the message record
        // exists for every package step, with or without a runner attached
        // (the payload then carries the raw declared params).
        let has_package = match execution {
            Some(execution) => self.step_uses(snapshot, &execution.model_node)?.is_some(),
            None => false,
        };
        let (facts, fresh) = if !has_package || created_exists {
            (act_facts(snapshot, execution_id), false)
        } else if let Some(execution) = execution {
            match self
                .resolve_step(snapshot, execution_id, &execution.model_node)
                .await
            {
                Ok(facts) => (facts, true),
                Err(error) => {
                    // The act cannot start with these params (the legacy act's
                    // `run()` fails it before it waits or executes). The state
                    // table, like the legacy sequence, only decides a failure
                    // from a started act: this hop marks it started and hands
                    // the decision to the effect hop, which resolves again and
                    // fails it — the same error walk, one hop later.
                    tracing::debug!(
                        pid = %snapshot.process_id,
                        node = %execution_id.task_id,
                        error = %error,
                        "act resolution failed at start; failing it on its effect hop"
                    );
                    if execution.state == crate::kernel::ExecutionState::Ready {
                        plan.state_changes.push(StateChange {
                            execution: execution_id.clone(),
                            expected_revision: execution.revision,
                            from: crate::kernel::ExecutionState::Ready,
                            to: crate::kernel::ExecutionState::Running,
                        });
                        if !action_exists {
                            plan.operations.push(OperationIntent {
                                id: action_id.clone(),
                                kind: OperationKind::ApplyAction,
                            });
                        }
                    }
                    return Ok(plan);
                }
            }
        } else {
            (None, false)
        };
        // The act message: every `uses` step announces itself to the client
        // when it starts (the legacy act's `Created` message — Irq waits on
        // it, Msg informs, Func pairs it with the later completion). The
        // durable record rides the outbox; the attached transport turns it
        // into the engine's message.
        if has_package {
            if let Some(facts) = &facts
                && fresh
            {
                plan.variable_writes.push(VariableWrite {
                    scope: execution_id.clone(),
                    name: PARAMS_VAR.to_string(),
                    value: facts.params.to_string(),
                });
                plan.variable_writes.push(VariableWrite {
                    scope: execution_id.clone(),
                    name: OPTIONS_VAR.to_string(),
                    value: facts.options.to_string(),
                });
            }
            if !created_exists
                && let Some(payload) = self.act_message_payload(
                    snapshot,
                    execution_id,
                    &id.task_id,
                    facts.as_ref(),
                    None,
                    None,
                )?
            {
                plan.outbox.push(crate::kernel::OutboxIntent {
                    key: created_key,
                    topic: "message.created".into(),
                    payload: payload.to_string(),
                });
            }
        }
        if !action_exists && auto {
            plan.operations.push(OperationIntent {
                id: action_id.clone(),
                kind: OperationKind::ApplyAction,
            });
        }
        if let Some(ready) =
            execution.filter(|ready| ready.state == crate::kernel::ExecutionState::Ready)
        {
            // Waiting packages park in `Waiting` (the legacy `Interrupt`);
            // everything else runs to completion.
            let target = if auto {
                crate::kernel::ExecutionState::Running
            } else {
                crate::kernel::ExecutionState::Waiting
            };
            plan.state_changes.push(StateChange {
                execution: execution_id.clone(),
                expected_revision: ready.revision,
                from: crate::kernel::ExecutionState::Ready,
                to: target,
            });
            // A node that declares `timeouts` starts its cost clock here and
            // arms the timer that fires them (see `kernel::timeout` and
            // `KernelHost::timeout_round`): the timer is durable, so a tick
            // after a crash still fires a branch whose window opened while the
            // host was down. Only timeout owners carry the stamp — it is a
            // kernel fact, never a variable a model reads.
            if !self
                .timeout_branches(snapshot, &ready.model_node)?
                .is_empty()
            {
                plan.variable_writes
                    .push(crate::kernel::timeout_start_write(execution_id, now_ms));
                let sequence = next_plan_sequence(
                    snapshot,
                    &plan,
                    &execution_id.task_id,
                    execution_id.generation,
                );
                plan.operations.push(OperationIntent {
                    id: OperationId {
                        process_id: id.process_id.clone(),
                        task_id: execution_id.task_id.clone(),
                        generation: execution_id.generation,
                        sequence,
                    },
                    kind: OperationKind::DispatchTimeout,
                });
            }
        }
        Ok(plan)
    }

    /// The bare hop plan: the operation's own commit plus its outbox note.
    /// State changes and successor operations are layered on by the callers.
    #[allow(clippy::too_many_arguments)]
    fn task_hop_plan(
        &self,
        snapshot: &crate::kernel::AggregateSnapshot,
        id: &OperationId,
        execution_id: &crate::kernel::ExecutionId,
        execution: Option<&crate::kernel::ExecutionSnapshot>,
        claim: &OperationCommit,
        note: &str,
        topic: &str,
    ) -> Result<TransitionPlan> {
        execution
            .ok_or_else(|| ActError::Runtime(format!("execution {execution_id:?} not found")))?;
        Ok(TransitionPlan {
            process_id: id.process_id.clone(),
            expected: snapshot.version.clone(),
            execution_creates: Vec::new(),
            state_changes: Vec::new(),
            variable_writes: Vec::new(),
            source_operation: Some(claim.clone()),
            operations: Vec::new(),
            outbox: vec![crate::kernel::OutboxIntent {
                key: format!("{note}:{}:{}:{}", id.process_id, id.task_id, id.generation),
                topic: topic.into(),
                payload: String::new(),
            }],
        })
    }

    /// The durable effect of one act completion. A plain execution completes
    /// and propagates; a container (a branched step, or a branch over its own
    /// steps) fans its children out instead and parks in `Waiting` until the
    /// join completes it. `action` is `None` for the auto-completion hop.
    /// Only a completion fans out — every other client decision (Submit /
    /// Skip / Cancel / Abort / Fail) is terminal for the container and its
    /// unborn children, exactly like the legacy engine, where a step that is
    /// no longer running never schedules its branches.
    ///
    /// On the auto-completion hop an attached effect runner runs the step's
    /// Func package (the legacy `ActRunAs::Func` run): outputs commit as
    /// scope variables with the same atomic completion, and a package error
    /// decides the execution `Failed` with the error variables — the legacy
    /// package-error path, whose error walk the propagation then carries.
    // Like `task_hop_plan`, the planner mirrors the aggregate's own facts
    // (operation, execution, claim) as separate parameters; bundling them
    // into a struct would only rename the same eight fields.
    #[allow(clippy::too_many_arguments)]
    async fn effect_plan(
        &self,
        snapshot: &crate::kernel::AggregateSnapshot,
        id: &OperationId,
        execution_id: &crate::kernel::ExecutionId,
        execution: &crate::kernel::ExecutionSnapshot,
        claim: &OperationCommit,
        action: Option<crate::kernel::ActionKind>,
        inputs: Vec<(String, String)>,
    ) -> Result<TransitionPlan> {
        // The step is already deciding a caught error (the sign its failure
        // hop pinned): no later hop may complete, fail or fan it out — the
        // catch join decides it. The client path refuses such an action at the
        // preflight; what reaches here is the stale auto hop a failure raced,
        // which keeps the operation's own commit as its bare remainder (the
        // same treatment the `ApplyAction` arm gives a decision that landed
        // mid-hop).
        if is_catch_decider(snapshot, execution_id) {
            return self.task_hop_plan(
                snapshot,
                id,
                execution_id,
                Some(execution),
                claim,
                "completed",
                "task.completed",
            );
        }
        match action {
            None => {
                let propagate_id = OperationId {
                    process_id: id.process_id.clone(),
                    task_id: id.task_id.clone(),
                    generation: id.generation,
                    sequence: id.sequence + 1,
                };
                // The act's own effect runs *before* its children are
                // dispatched: the legacy act completes (or fails) first, and
                // a `catches` handler answers the act's failure — never a
                // failure of the children it already dispatched.
                let facts = act_facts(snapshot, execution_id);
                let mut effect: Option<effects::EffectOutcome> = None;
                if let Some((runner, request)) =
                    self.step_effect(snapshot, execution_id, &execution.model_node)?
                {
                    match runner.run(request).await {
                        Ok(outcome) => effect = Some(outcome),
                        Err(error) => {
                            // A package error takes the failure path, where a
                            // step declaring `catches` keeps running and
                            // dispatches them (the legacy `Step::on_error`).
                            if let Some(plan) = self
                                .catch_fanout(
                                    snapshot,
                                    claim,
                                    execution_id,
                                    execution,
                                    &package_failure(&error),
                                    &inputs,
                                )
                                .await?
                            {
                                return Ok(plan);
                            }
                            return self
                                .package_failure_plan(
                                    snapshot,
                                    id,
                                    execution_id,
                                    execution,
                                    claim,
                                    &error,
                                )
                                .await;
                        }
                    }
                }
                // A container dispatches its children instead of completing —
                // with the act's own outcome folded into the same atomic
                // commit. A leaf completes and propagates.
                if let Some(command) = self
                    .fanout_command(
                        snapshot,
                        execution_id,
                        claim,
                        &execution.model_node,
                        &inputs,
                    )
                    .await?
                {
                    let mut plan = Self::container_fanout_plan(snapshot, &command)?;
                    if let Some(outcome) = &effect {
                        self.fold_effect_outcome(snapshot, execution_id, outcome, &mut plan);
                        plan.validate().map_err(runtime)?;
                    }
                    return Ok(plan);
                }
                // A step whose timeout branches are still in flight may not
                // complete yet: the legacy `auto_complete` counts every child
                // task, the timeout branches included. The act's own completion
                // is recorded and the execution waits for them (the container
                // join finishes it when the last one settles).
                if self.waits_for_timeouts(snapshot, execution_id, execution)? {
                    let mut plan = self.park_for_children_plan(
                        snapshot,
                        id,
                        execution_id,
                        execution,
                        claim,
                        &inputs,
                    )?;
                    if let Some(outcome) = &effect {
                        self.fold_effect_outcome(snapshot, execution_id, outcome, &mut plan);
                        plan.validate().map_err(runtime)?;
                    }
                    return Ok(plan);
                }
                let mut plan = self.task_hop_plan(
                    snapshot,
                    id,
                    execution_id,
                    Some(execution),
                    claim,
                    "completed",
                    "task.completed",
                )?;
                if let Some(outcome) = &effect {
                    self.fold_effect_outcome(snapshot, execution_id, outcome, &mut plan);
                }
                // The act's Completed message rides the completion record.
                if let Some(payload) = self.act_message_payload(
                    snapshot,
                    execution_id,
                    &id.task_id,
                    facts.as_ref(),
                    Some(&plan.variable_writes),
                    None,
                )? {
                    plan.outbox[0].payload = payload.to_string();
                }
                plan.state_changes.push(StateChange {
                    execution: execution_id.clone(),
                    expected_revision: execution.revision,
                    from: execution.state,
                    to: crate::kernel::ExecutionState::Completed,
                });
                if !snapshot.operations.contains_key(&propagate_id) {
                    plan.operations.push(OperationIntent {
                        id: propagate_id,
                        kind: OperationKind::PropagateTask,
                    });
                }
                Ok(plan)
            }
            Some(kind) => {
                // A client completion of a container dispatches its children
                // instead of completing it (the legacy `Next` on a running
                // step) — the step declaring `catches` included: the completion
                // is its *act* completing, which then runs into its handlers.
                if matches!(kind, crate::kernel::ActionKind::Complete)
                    && let Some(command) = self
                        .fanout_command(
                            snapshot,
                            execution_id,
                            claim,
                            &execution.model_node,
                            &inputs,
                        )
                        .await?
                {
                    return Self::container_fanout_plan(snapshot, &command);
                }
                // A completion of a step whose timeout branches are still in
                // flight is its *act* completing, not the step: the execution
                // waits for the branches it fired (see `park_for_children_plan`).
                if matches!(kind, crate::kernel::ActionKind::Complete)
                    && self.waits_for_timeouts(snapshot, execution_id, execution)?
                {
                    return self.park_for_children_plan(
                        snapshot,
                        id,
                        execution_id,
                        execution,
                        claim,
                        &inputs,
                    );
                }
                // A client's error on a step declaring `catches` is the legacy
                // `Step::on_error`: the step stays running and dispatches its
                // handlers instead of being decided `Failed` here.
                if let crate::kernel::ActionKind::Fail { code, message } = &kind
                    && let Some(plan) = self
                        .catch_fanout(
                            snapshot,
                            claim,
                            execution_id,
                            execution,
                            &Failure::new(code.clone(), message.clone()),
                            &inputs,
                        )
                        .await?
                {
                    return Ok(plan);
                }
                let command = crate::kernel::ApplyActionCommand {
                    process_id: id.process_id.clone(),
                    execution: execution_id.clone(),
                    action: kind,
                    inputs,
                    source_operation: claim.clone(),
                };
                let mut plan =
                    crate::kernel::ActionPlanner::plan(snapshot, &command).map_err(runtime)?;
                // A client action that decides a `uses` execution announces it
                // on the act's own message, exactly like the legacy act.
                self.attach_act_message(snapshot, execution_id, &mut plan)?;
                Ok(plan)
            }
        }
    }

    /// The `(uses, raw params, node name, model id)` of a `uses` step node,
    /// from the durable model — the act message's and the effect seam's
    /// package facts.
    fn step_uses(
        &self,
        snapshot: &crate::kernel::AggregateSnapshot,
        model_node: &crate::kernel::NodeId,
    ) -> Result<Option<(String, serde_json::Value, String, String)>> {
        let Some(model) = model_of(snapshot)? else {
            return Ok(None);
        };
        let Some(step) = find_step_in_model(&model, model_node.as_str()) else {
            return Ok(None);
        };
        let Some(uses) = step.uses.clone().filter(|uses| !uses.trim().is_empty()) else {
            return Ok(None);
        };
        Ok(Some((
            uses,
            step.params.clone(),
            step.name.clone(),
            model.id.clone(),
        )))
    }

    /// The act message payload of one `uses` step: the fact set the driver's
    /// transport maps onto the engine's `Message`, faithful to the legacy
    /// `Task::create_message` — the resolved params and options, the step and
    /// model inputs, the scope inputs the message carries, then the hop's own
    /// outputs or its error. `facts` are the start hop's pinned resolution
    /// (absent for a host without a runner or an aggregate seeded before it);
    /// `outputs` are the hop's own scope writes (`__`-private facts and the
    /// error variables excluded); `error` is the failure the hop decided.
    #[allow(clippy::too_many_arguments)]
    fn act_message_payload(
        &self,
        snapshot: &crate::kernel::AggregateSnapshot,
        execution_id: &crate::kernel::ExecutionId,
        task_id: &str,
        facts: Option<&ActFacts>,
        outputs: Option<&[VariableWrite]>,
        error: Option<(&str, &str)>,
    ) -> Result<Option<serde_json::Value>> {
        let node = crate::kernel::NodeId::parse(task_id).map_err(runtime)?;
        let Some((uses, raw_params, name, mid)) = self.step_uses(snapshot, &node)? else {
            return Ok(None);
        };
        // The resolved facts when the start hop pinned them, the raw
        // declaration otherwise (`params` before a runner ever saw the step).
        let params = facts
            .map(|facts| facts.params.clone())
            .unwrap_or(raw_params);
        let options = facts
            .map(|facts| facts.options.clone())
            .unwrap_or(serde_json::Value::Null);

        let mut inputs = input_vars(snapshot, execution_id);
        // The act folds into its step node in the kernel, so the step the
        // legacy message pointed at is the node itself.
        inputs.insert(
            crate::utils::consts::STEP_KEY.to_string(),
            serde_json::json!({
                crate::utils::consts::STEP_NODE_ID: task_id,
                crate::utils::consts::STEP_NODE_NAME: name,
                crate::utils::consts::STEP_TASK_ID: task_id,
            }),
        );
        if let Some(model) = model_of(snapshot)? {
            inputs.insert(
                crate::utils::consts::WORKFLOW_MODEL_KEY.to_string(),
                serde_json::json!({
                    "id": model.id,
                    "name": model.name,
                    "options": model.options,
                }),
            );
        }
        inputs.insert(crate::utils::consts::ACT_PARAMS_KEY.to_string(), params);
        inputs.insert(crate::utils::consts::ACT_OPTIONS_KEY.to_string(), options);

        let mut payload = serde_json::json!({
            "nid": task_id,
            "name": name,
            "uses": uses,
            "mid": mid,
            "inputs": serde_json::Value::Object(inputs),
        });
        if let Some((code, message)) = error {
            payload["error"] = serde_json::json!({ "code": code, "message": message });
        } else if let Some(outputs) = outputs {
            let vars: serde_json::Map<String, serde_json::Value> = outputs
                .iter()
                .filter(|write| {
                    !crate::utils::consts::is_private_key(&write.name)
                        && !write.name.starts_with("error.")
                })
                .map(|write| {
                    (
                        write.name.clone(),
                        serde_json::from_str(&write.value).unwrap_or(serde_json::Value::Null),
                    )
                })
                .collect();
            payload["outputs"] = serde_json::Value::Object(vars);
        }
        Ok(Some(payload))
    }

    /// The variable space the evaluation seams see for one execution: its
    /// scope chain (nearest wins), the run's durable env and its caller
    /// authority, on the acting node of the declaring model.
    fn scope_spec(
        &self,
        snapshot: &crate::kernel::AggregateSnapshot,
        execution_id: &crate::kernel::ExecutionId,
        model: crate::model::Workflow,
    ) -> Result<scope::ScopeSpec> {
        Ok(scope::ScopeSpec {
            process_id: snapshot.process_id.clone(),
            task_id: execution_id.task_id.clone(),
            model,
            vars: scope_vars(snapshot, execution_id),
            env: projection::env_map(snapshot)?,
            owner: owner_of(snapshot)?,
        })
    }

    /// The effect request for a `uses` step, when a runner is attached:
    /// `None` for a leaf without a package or a host running without the seam.
    fn step_effect(
        &self,
        snapshot: &crate::kernel::AggregateSnapshot,
        execution_id: &crate::kernel::ExecutionId,
        model_node: &crate::kernel::NodeId,
    ) -> Result<Option<(Arc<dyn effects::EffectRunner>, effects::EffectRequest)>> {
        let Some(runner) = self.effect.read().clone() else {
            return Ok(None);
        };
        let Some(model) = model_of(snapshot)? else {
            return Ok(None);
        };
        let Some(step) = find_step_in_model(&model, model_node.as_str()) else {
            return Ok(None);
        };
        let Some(package) = step.uses.clone().filter(|uses| !uses.trim().is_empty()) else {
            return Ok(None);
        };
        Ok(Some((
            runner,
            effects::EffectRequest {
                scope: self.scope_spec(snapshot, execution_id, model)?,
                package,
            },
        )))
    }

    /// Resolve one `uses` step's package at start (the legacy act's `run()`
    /// gate): the package's `run_as` and its validated params. A resolution
    /// error is the legacy act's start error.
    async fn resolve_step(
        &self,
        snapshot: &crate::kernel::AggregateSnapshot,
        execution_id: &crate::kernel::ExecutionId,
        model_node: &crate::kernel::NodeId,
    ) -> Result<Option<ActFacts>> {
        let Some((runner, request)) = self.step_effect(snapshot, execution_id, model_node)? else {
            return Ok(None);
        };
        let resolved = runner.resolve(&request).await?;
        Ok(Some(ActFacts {
            params: resolved.params,
            options: resolved.options,
        }))
    }

    /// The declared timeout branches of one model node, in declaration order.
    ///
    /// An empty list means the node's step declares no `timeouts`: the timer
    /// has nothing to fire (`Retire`). Each branch carries the durable facts a
    /// dispatch needs; its guard stays in the model and is evaluated by the
    /// round.
    fn timeout_branches(
        &self,
        snapshot: &crate::kernel::AggregateSnapshot,
        model_node: &crate::kernel::NodeId,
    ) -> Result<Vec<crate::kernel::TimeoutBranch>> {
        let Some(model) = model_of(snapshot)? else {
            return Ok(Vec::new());
        };
        let Some(step) = find_step_in_model(&model, model_node.as_str()) else {
            return Ok(Vec::new());
        };
        let mut branches = Vec::with_capacity(step.timeouts.len());
        for timeout in &step.timeouts {
            branches.push(crate::kernel::TimeoutBranch {
                task_id: timeout.id.clone(),
                model_node: node_id(&timeout.id)?,
                vars: timeout
                    .vars
                    .iter()
                    .map(|var| (var.name.clone(), var.value.to_string()))
                    .collect(),
                auto: auto_decision(snapshot, &timeout.id)?,
            });
        }
        Ok(branches)
    }

    /// The `if` of one declared branch, from the declaring model: the window the
    /// tick evaluates (`$cost_in(...)` in the models this ports). A branch
    /// without a guard fires as soon as the round reaches it — a legacy branch
    /// whose `Step::init` had nothing to test.
    fn timeout_guard(
        &self,
        snapshot: &crate::kernel::AggregateSnapshot,
        branch: &str,
    ) -> Result<Option<String>> {
        let Some(model) = model_of(snapshot)? else {
            return Ok(None);
        };
        Ok(find_step_in_model(&model, branch).and_then(|step| step.r#if.clone()))
    }

    /// One dispatch round of a timed-out execution: the legacy tick's
    /// `Step::on_timeout` + `dispatch_next_timeout_branch` + the branch's own
    /// `Step::init` guard, decided against the aggregate.
    ///
    /// The cost is what passed since the execution's own start hop (the legacy
    /// task's creation stamp); it is frozen into the branch's evaluation as the
    /// `__cost` task datum `$cost()`/`$cost_in(...)` read.
    async fn timeout_round(
        &self,
        snapshot: &crate::kernel::AggregateSnapshot,
        owner: &crate::kernel::ExecutionId,
        execution: Option<&crate::kernel::ExecutionSnapshot>,
        now_ms: i64,
    ) -> Result<TimeoutRound> {
        // Only a running or waiting execution times out (the legacy tick's
        // `is_timeouts() && (running || interrupted)`); anything else — a
        // re-entered generation's stale timer included — has nothing left.
        let Some(execution) = execution.filter(|execution| {
            matches!(
                execution.state,
                crate::kernel::ExecutionState::Running | crate::kernel::ExecutionState::Waiting
            )
        }) else {
            return Ok(TimeoutRound::Retire);
        };
        let branches = self.timeout_branches(snapshot, &execution.model_node)?;
        if branches.is_empty() {
            return Ok(TimeoutRound::Retire);
        }
        let cost_ms = crate::kernel::timeout_cost_ms(
            now_ms,
            snapshot
                .variables
                .get(&(owner.clone(), crate::kernel::STARTED_VAR.to_string()))
                .map(String::as_str),
        );
        match crate::kernel::timeout_candidate(snapshot, owner, &branches) {
            crate::kernel::TimeoutCandidate::None => Ok(TimeoutRound::Retire),
            crate::kernel::TimeoutCandidate::Blocked => Ok(TimeoutRound::Idle),
            crate::kernel::TimeoutCandidate::Branch(index) => {
                let branch = branches[index].clone();
                let guard = match self.timeout_guard(snapshot, &branch.task_id)? {
                    None => TimeoutGuard::Fire,
                    Some(expr) => {
                        // The window is decided on the engine's own
                        // environment (the legacy branch task's `init`), with
                        // the frozen cost the legacy `on_timeout` wrote into
                        // the task data.
                        let predicate = self
                            .evaluate_branch_predicate(
                                snapshot,
                                owner,
                                &expr,
                                &[(
                                    crate::utils::consts::TASK_COST.to_string(),
                                    cost_ms.to_string(),
                                )],
                            )
                            .await?;
                        match predicate {
                            crate::kernel::PredicateValue::True => TimeoutGuard::Fire,
                            // The window is not open: no claim, the round ends
                            // (the legacy branch skips without claiming, so a
                            // later round re-evaluates it).
                            crate::kernel::PredicateValue::False => return Ok(TimeoutRound::Idle),
                            // A guard that cannot be evaluated is the legacy
                            // branch's `init` error: the branch is admitted and
                            // fails on its first hop.
                            crate::kernel::PredicateValue::Null => TimeoutGuard::Fail(format!(
                                "timeout step '{}' guard evaluated to null",
                                branch.task_id
                            )),
                            crate::kernel::PredicateValue::Invalid { message } => {
                                TimeoutGuard::Fail(message)
                            }
                        }
                    }
                };
                Ok(TimeoutRound::Dispatch {
                    branch,
                    guard,
                    cost_ms,
                    declared: branches.len(),
                })
            }
        }
    }

    /// The plan of one dispatch round: the due branch admitted `Entry` through
    /// the container machinery (its execution, the owner's frozen cost and the
    /// advanced one-shot marker in one atomic commit) plus the timer's own
    /// replacement intent, so a later window still fires.
    #[allow(clippy::too_many_arguments)]
    fn timeout_plan(
        &self,
        snapshot: &crate::kernel::AggregateSnapshot,
        id: &OperationId,
        execution_id: &crate::kernel::ExecutionId,
        execution: Option<&crate::kernel::ExecutionSnapshot>,
        claim: &OperationCommit,
        branch: crate::kernel::TimeoutBranch,
        guard: TimeoutGuard,
        cost_ms: i64,
        declared: usize,
    ) -> Result<TransitionPlan> {
        execution
            .ok_or_else(|| ActError::Runtime(format!("execution {execution_id:?} not found")))?;
        let mut pinned = Vec::new();
        if let TimeoutGuard::Fail(message) = guard {
            pinned.push((
                BRANCH_ERROR_VAR.to_string(),
                serde_json::json!({ "code": "", "message": message }).to_string(),
            ));
        }
        let claimed_after = crate::kernel::timeout_claim(
            &crate::kernel::timeout_claimed(snapshot, execution_id),
            &branch.task_id,
        );
        let dispatch = crate::kernel::timeout_dispatch(
            snapshot,
            execution_id,
            claim.clone(),
            &branch,
            pinned,
            &claimed_after,
            cost_ms,
        );
        let mut plan = crate::kernel::ContainerPlanner::plan_fanout(snapshot, &dispatch.command)
            .map_err(runtime)?;
        if dispatch.claimed_after.len() < declared {
            // Declared branches remain unfired: the timer re-arms for them
            // instead of retiring (a window that has not opened yet must still
            // fire, and only a fresh round can see it).
            let sequence = next_plan_sequence(
                snapshot,
                &plan,
                &execution_id.task_id,
                execution_id.generation,
            );
            plan.operations.push(OperationIntent {
                id: OperationId {
                    process_id: id.process_id.clone(),
                    task_id: execution_id.task_id.clone(),
                    generation: execution_id.generation,
                    sequence,
                },
                kind: OperationKind::DispatchTimeout,
            });
        }
        plan.validate().map_err(runtime)?;
        Ok(plan)
    }

    /// The bare remainder of an operation with nothing to do: its own commit
    /// and nothing else. Completing the operation is the whole effect.
    fn empty_remainder(
        snapshot: &crate::kernel::AggregateSnapshot,
        id: &OperationId,
        claim: &OperationCommit,
    ) -> TransitionPlan {
        TransitionPlan {
            process_id: id.process_id.clone(),
            expected: snapshot.version.clone(),
            execution_creates: Vec::new(),
            state_changes: Vec::new(),
            variable_writes: Vec::new(),
            source_operation: Some(claim.clone()),
            operations: Vec::new(),
            outbox: Vec::new(),
        }
    }

    /// Whether an execution may not complete yet because a timeout branch it
    /// fired is still in flight. The legacy `auto_complete` counts every child
    /// task — the timeout branches included — so a step whose act is done waits
    /// for them; the kernel parks the execution in `Waiting` (a waiting act is
    /// already parked) and the container join finishes it.
    fn waits_for_timeouts(
        &self,
        snapshot: &crate::kernel::AggregateSnapshot,
        execution_id: &crate::kernel::ExecutionId,
        execution: &crate::kernel::ExecutionSnapshot,
    ) -> Result<bool> {
        let branches = self.timeout_branches(snapshot, &execution.model_node)?;
        Ok(!branches.is_empty()
            && crate::kernel::timeout_has_live_children(snapshot, execution_id, &branches))
    }

    /// The container fan-out plan: [`ContainerPlanner::plan_fanout`] plus the
    /// park mark the join reads.
    ///
    /// The mark belongs to this path, not to the planner: the container path is
    /// the one that parks an execution for the children it admitted, while a
    /// timeout dispatch admits a branch under an execution that is still
    /// waiting for its own act — marking that one would let the branch's
    /// settlement complete a step nobody answered (see
    /// [`crate::kernel::PARK_CHILDREN`]).
    fn container_fanout_plan(
        snapshot: &crate::kernel::AggregateSnapshot,
        command: &crate::kernel::ContainerFanoutCommand,
    ) -> Result<TransitionPlan> {
        let mut plan =
            crate::kernel::ContainerPlanner::plan_fanout(snapshot, command).map_err(runtime)?;
        // A container without children completes in this very hop: it never
        // parks, so it carries no mark.
        if !command.children.is_empty() {
            mark_parked_for_children(&mut plan, &command.source);
        }
        plan.validate().map_err(runtime)?;
        Ok(plan)
    }

    /// Record that `execution` parked in `Waiting` for its children. The
    /// planner's own fact wins over a client input of the same private name.
    fn park_for_children_plan(
        &self,
        snapshot: &crate::kernel::AggregateSnapshot,
        id: &OperationId,
        execution_id: &crate::kernel::ExecutionId,
        execution: &crate::kernel::ExecutionSnapshot,
        claim: &OperationCommit,
        inputs: &[(String, String)],
    ) -> Result<TransitionPlan> {
        let mut plan = self.task_hop_plan(
            snapshot,
            id,
            execution_id,
            Some(execution),
            claim,
            "completed",
            "task.completed",
        )?;
        for (name, value) in inputs {
            plan.variable_writes.push(VariableWrite {
                scope: execution_id.clone(),
                name: name.clone(),
                value: value.clone(),
            });
        }
        if execution.state == crate::kernel::ExecutionState::Running {
            plan.state_changes.push(StateChange {
                execution: execution_id.clone(),
                expected_revision: execution.revision,
                from: crate::kernel::ExecutionState::Running,
                to: crate::kernel::ExecutionState::Waiting,
            });
        }
        mark_parked_for_children(&mut plan, execution_id);
        // The act did complete: its message rides the record, exactly like a
        // leaf completion's.
        if let Some(payload) = self.act_message_payload(
            snapshot,
            execution_id,
            &id.task_id,
            act_facts(snapshot, execution_id).as_ref(),
            Some(&plan.variable_writes),
            None,
        )? {
            plan.outbox[0].payload = payload.to_string();
        }
        plan.validate().map_err(runtime)?;
        Ok(plan)
    }

    /// Fold one package outcome into the hop's plan: the returned outputs and
    /// the headless task's own data become the acting scope's writes, and the
    /// process env the package set merges into the run's durable env map on
    /// the root scope. All of it commits with the completion, so a crash
    /// before the commit re-runs the package against the same pre-state.
    fn fold_effect_outcome(
        &self,
        snapshot: &crate::kernel::AggregateSnapshot,
        execution_id: &crate::kernel::ExecutionId,
        outcome: &effects::EffectOutcome,
        plan: &mut TransitionPlan,
    ) {
        if let Some(outputs) = &outcome.outputs {
            for (name, value) in effects::outputs_as_writes(outputs) {
                plan.variable_writes.push(VariableWrite {
                    scope: execution_id.clone(),
                    name,
                    value,
                });
            }
        }
        for (name, value) in &outcome.data {
            plan.variable_writes.push(VariableWrite {
                scope: execution_id.clone(),
                name: name.clone(),
                value: value.to_string(),
            });
        }
        if !outcome.env.is_empty() {
            let Some(root) = root_execution(snapshot) else {
                return;
            };
            let mut map: serde_json::Map<String, serde_json::Value> = projection::env_map(snapshot)
                .unwrap_or_default()
                .into_iter()
                .collect();
            for (name, value) in &outcome.env {
                map.insert(name.clone(), value.clone());
            }
            plan.variable_writes.push(VariableWrite {
                scope: root,
                name: projection::ENV_MAP_VAR.to_string(),
                value: serde_json::Value::Object(map).to_string(),
            });
        }
    }

    /// The plan of an execution failure: the execution decided `Failed` with
    /// the error variables, its message on its own record, and the propagation
    /// that carries the error walk upward (the legacy `emit_error` path). An
    /// execution that already left the running states keeps its state — the
    /// bare hop remainder completes the operation. A step declaring `catches`
    /// never reaches the `Failed` decision: [`Self::catch_fanout`] keeps it
    /// running and dispatches its handlers instead.
    #[allow(clippy::too_many_arguments)]
    async fn failure_plan(
        &self,
        snapshot: &crate::kernel::AggregateSnapshot,
        id: &OperationId,
        execution_id: &crate::kernel::ExecutionId,
        execution: &crate::kernel::ExecutionSnapshot,
        claim: &OperationCommit,
        failure: &Failure,
    ) -> Result<TransitionPlan> {
        if let Some(plan) = self
            .catch_fanout(snapshot, claim, execution_id, execution, failure, &[])
            .await?
        {
            return Ok(plan);
        }
        let propagate_id = OperationId {
            process_id: id.process_id.clone(),
            task_id: id.task_id.clone(),
            generation: id.generation,
            sequence: id.sequence + 1,
        };
        let mut plan = self.task_hop_plan(
            snapshot,
            id,
            execution_id,
            Some(execution),
            claim,
            "failed",
            "task.failed",
        )?;
        if execution.state.is_terminal() {
            return Ok(plan);
        }
        plan.state_changes.push(StateChange {
            execution: execution_id.clone(),
            expected_revision: execution.revision,
            from: execution.state,
            to: crate::kernel::ExecutionState::Failed,
        });
        for (name, value) in [
            ("error.code", failure.code.clone()),
            ("error.message", failure.message.clone()),
        ] {
            plan.variable_writes.push(VariableWrite {
                scope: execution_id.clone(),
                name: name.to_string(),
                value,
            });
        }
        if !snapshot.operations.contains_key(&propagate_id) {
            plan.operations.push(OperationIntent {
                id: propagate_id,
                kind: OperationKind::PropagateTask,
            });
        }
        if let Some(payload) = self.act_message_payload(
            snapshot,
            execution_id,
            &id.task_id,
            act_facts(snapshot, execution_id).as_ref(),
            None,
            Some((&failure.code, &failure.message)),
        )? {
            plan.outbox[0].payload = payload.to_string();
        }
        Ok(plan)
    }

    /// The plan of a package/params failure: the error the effect seam
    /// reports, under the kernel's package error code.
    async fn package_failure_plan(
        &self,
        snapshot: &crate::kernel::AggregateSnapshot,
        id: &OperationId,
        execution_id: &crate::kernel::ExecutionId,
        execution: &crate::kernel::ExecutionSnapshot,
        claim: &OperationCommit,
        error: &crate::ActError,
    ) -> Result<TransitionPlan> {
        self.failure_plan(
            snapshot,
            id,
            execution_id,
            execution,
            claim,
            &package_failure(error),
        )
        .await
    }

    /// The plan of a caught failure: the legacy `Step::on_error`, which keeps
    /// a failed step `Running`, pins the error variables its CATCH predicates
    /// read (`$ecode()` answers the legacy `ecode`; the ERROR sign marks the
    /// step as deciding), and dispatches every `catches` child as a container
    /// child. The step's own join ([`crate::kernel::ContainerPlanner::plan_catch_join`])
    /// decides it once they settled: every handler skipped resumes the error,
    /// anything else catches it.
    ///
    /// `None` when the decision is not a catchable one — the model node
    /// declares no `catches`, the execution is not running, or the step is
    /// already deciding a caught error (the legacy guard on
    /// `task.sign().is_none()`: a second failure skips the dispatch and walks
    /// the error up instead).
    #[allow(clippy::too_many_arguments)]
    async fn catch_fanout(
        &self,
        snapshot: &crate::kernel::AggregateSnapshot,
        claim: &OperationCommit,
        execution_id: &crate::kernel::ExecutionId,
        execution: &crate::kernel::ExecutionSnapshot,
        failure: &Failure,
        inputs: &[(String, String)],
    ) -> Result<Option<TransitionPlan>> {
        if !matches!(
            execution.state,
            crate::kernel::ExecutionState::Running | crate::kernel::ExecutionState::Waiting
        ) {
            return Ok(None);
        }
        let Some(model) = model_of(snapshot)? else {
            return Ok(None);
        };
        let Some(step) = find_step_in_model(&model, execution.model_node.as_str()) else {
            return Ok(None);
        };
        if step.catches.is_empty() || is_catch_decider(snapshot, execution_id) {
            return Ok(None);
        }
        // The error facts of the failed act, as the legacy task data carried
        // them: the handler's scope inherits them, so its guard and its params
        // select on `$ecode()` / `message` exactly like the legacy run. The
        // aggregate's variable representation is JSON text (a raw string would
        // not decode into the headless scope a predicate evaluates on), and
        // the failure — never the caller's copy — is what the act's data held.
        let mut overlay: Vec<(String, String)> = inputs
            .iter()
            .filter(|(name, _)| {
                !matches!(
                    name.as_str(),
                    "ecode" | "message" | "error.code" | "error.message"
                )
            })
            .cloned()
            .collect();
        overlay.push(("ecode".to_string(), json_var(&failure.ecode)));
        overlay.push(("message".to_string(), json_var(&failure.message)));
        let mut children = Vec::with_capacity(step.catches.len());
        for catch in &step.catches {
            let mut vars: Vec<(String, String)> = catch
                .vars
                .iter()
                .map(|var| (var.name.clone(), var.value.to_string()))
                .collect();
            // The legacy `TASK_SIGN = CATCH` the dispatch seeds into the
            // handler task: the catch join reads it back from the aggregate.
            vars.push((
                crate::kernel::SIGN_VAR.to_string(),
                crate::kernel::SIGN_CATCH.to_string(),
            ));
            let mut plan = crate::kernel::ContainerChildPlan::Entry;
            if let Some(expr) = &catch.r#if {
                // The handler's own selection (the legacy `Step::init`
                // guard), evaluated with the error the step is deciding.
                match self
                    .evaluate_branch_predicate(snapshot, execution_id, expr, &overlay)
                    .await?
                {
                    PredicateValue::True => {}
                    PredicateValue::False => plan = crate::kernel::ContainerChildPlan::Skip,
                    PredicateValue::Null => vars.push((
                        BRANCH_ERROR_VAR.to_string(),
                        serde_json::json!({
                            "code": "",
                            "message": format!("catch step '{}' guard evaluated to null", catch.id)
                        })
                        .to_string(),
                    )),
                    PredicateValue::Invalid { message } => vars.push((
                        BRANCH_ERROR_VAR.to_string(),
                        serde_json::json!({ "code": "", "message": message }).to_string(),
                    )),
                }
            }
            children.push(crate::kernel::ContainerChild {
                task_id: catch.id.clone(),
                model_node: node_id(&catch.id)?,
                plan,
                predecessors: vec![crate::kernel::ContainerPredecessor::Source],
                vars,
                auto: auto_decision(snapshot, &catch.id)?,
            });
        }
        // The step keeps running while its handlers work; the client's own
        // inputs land on its scope like any completion's — minus the error
        // names written explicitly below.
        let reserved = ["ecode", "message", "error.code", "error.message"];
        let inputs: Vec<(String, String)> = inputs
            .iter()
            .filter(|(name, _)| !reserved.contains(&name.as_str()))
            .cloned()
            .collect();
        let command = crate::kernel::ContainerFanoutCommand {
            process_id: snapshot.process_id.clone(),
            source: execution_id.clone(),
            source_operation: claim.clone(),
            children,
            inputs,
            source_park: crate::kernel::ExecutionState::Running,
        };
        let mut plan =
            crate::kernel::ContainerPlanner::plan_fanout(snapshot, &command).map_err(runtime)?;
        // The error facts of the failed act: the legacy task-data names a
        // catch predicate selects with (`$ecode()` / `message`) and the
        // kernel's own error variables, which the resumed error keeps
        // carrying up. Values are JSON text, the aggregate's representation.
        for (name, value) in [
            ("ecode".to_string(), json_var(&failure.ecode)),
            ("message".to_string(), json_var(&failure.message)),
            ("error.code".to_string(), json_var(&failure.code)),
            ("error.message".to_string(), json_var(&failure.message)),
            (
                crate::kernel::SIGN_VAR.to_string(),
                // The sign is an engine fact (`__`-private, never part of a
                // predicate's scope), so it is stored raw — the same form the
                // fan-out writes on the handler children, which the catch join
                // compares it against.
                crate::kernel::SIGN_ERROR.to_string(),
            ),
        ] {
            plan.variable_writes.push(VariableWrite {
                scope: execution_id.clone(),
                name,
                value,
            });
        }
        plan.validate().map_err(runtime)?;
        // The failed act's own message: the legacy act emitted `Error` before
        // the step recovered, so the client sees the failure the handler is
        // answering. Sequence-qualified, so the step's own later decision (a
        // resumed error) carries a record of its own.
        if let Some(payload) = self.act_message_payload(
            snapshot,
            execution_id,
            &execution_id.task_id,
            act_facts(snapshot, execution_id).as_ref(),
            None,
            Some((&failure.code, &failure.message)),
        )? {
            plan.outbox.push(crate::kernel::OutboxIntent {
                key: format!(
                    "failed:{}:{}:{}:{}",
                    snapshot.process_id,
                    execution_id.task_id,
                    execution_id.generation,
                    claim.id.sequence
                ),
                topic: "task.failed".into(),
                payload: payload.to_string(),
            });
            plan.validate().map_err(runtime)?;
        }
        Ok(Some(plan))
    }

    /// Announce a `uses` execution's decided state on the planner's own outbox
    /// record: the legacy act emitted one message per state transition, so a
    /// client action that decides the act (complete/submit/skip/cancel/abort/
    /// fail) must speak the same message. The record's topic already names the
    /// state; its opaque lifecycle payload becomes the act payload.
    fn attach_act_message(
        &self,
        snapshot: &crate::kernel::AggregateSnapshot,
        execution_id: &crate::kernel::ExecutionId,
        plan: &mut TransitionPlan,
    ) -> Result<()> {
        let Some(change) = plan
            .state_changes
            .iter()
            .find(|change| &change.execution == execution_id)
        else {
            return Ok(());
        };
        let Some(execution) = snapshot.executions.get(execution_id) else {
            return Ok(());
        };
        let Some(topic) = message_topic(change.to) else {
            return Ok(());
        };
        if self.step_uses(snapshot, &execution.model_node)?.is_none() {
            return Ok(());
        }
        // The action's own writes carry the error variables a `Fail` action
        // supplies; later writes win, like the commit's own order.
        let read = |name: &str| {
            plan.variable_writes
                .iter()
                .rev()
                .find(|write| write.name == name)
                .map(|write| json_text(&write.value))
                .unwrap_or_default()
        };
        let error = (change.to == crate::kernel::ExecutionState::Failed)
            .then(|| (read("error.code"), read("error.message")));
        let error = error
            .as_ref()
            .map(|(code, message)| (code.as_str(), message.as_str()));
        let facts = act_facts(snapshot, execution_id);
        if let Some(payload) = self.act_message_payload(
            snapshot,
            execution_id,
            &execution_id.task_id,
            facts.as_ref(),
            Some(&plan.variable_writes),
            error,
        )? {
            match plan.outbox.first_mut() {
                Some(record) => record.payload = payload.to_string(),
                None => plan.outbox.push(crate::kernel::OutboxIntent {
                    key: format!(
                        "act:{}:{}:{}",
                        plan.process_id, execution_id.task_id, execution_id.generation
                    ),
                    topic: topic.to_string(),
                    payload: payload.to_string(),
                }),
            }
        }
        Ok(())
    }

    /// The fan-out command for a container execution, or `None` when the
    /// model node is a plain leaf. Branch selection goes through the kernel's
    /// own `BranchPlanner::decide`; the inner chain of a branch is queued
    /// behind its first step.
    async fn fanout_command(
        &self,
        snapshot: &crate::kernel::AggregateSnapshot,
        execution_id: &crate::kernel::ExecutionId,
        claim: &OperationCommit,
        model_node: &crate::kernel::NodeId,
        inputs: &[(String, String)],
    ) -> Result<Option<ContainerFanoutCommand>> {
        let Some(model) = model_of(snapshot)? else {
            return Ok(None);
        };
        let Some(shape) = container_shape(&model, model_node) else {
            return Ok(None);
        };
        // A container whose children already left `Created` has fanned out
        // for this generation; a completion now is a force-finish (the
        // legacy client `Next` on a running step), not a second fan-out. A
        // rewound container's recreated children are all `Created` — exactly
        // like a never-fanned one — so they fan out (and readmit) again.
        //
        // Only the shape's own children count: a step that declares both
        // `branches` and `timeouts` has a live timeout branch under it as soon
        // as the tick fires one, and that branch must not read as "already
        // fanned out" — the legacy step schedules its branch children when it
        // runs into them, whatever a timeout did meanwhile.
        let declared: Vec<&str> = match &shape {
            ContainerShape::Branches(branches) => {
                branches.iter().map(|branch| branch.id.as_str()).collect()
            }
            ContainerShape::Catches(catches) => {
                catches.iter().map(|catch| catch.id.as_str()).collect()
            }
            ContainerShape::Steps(steps) => steps.iter().map(|step| step.id.as_str()).collect(),
        };
        if snapshot.executions.values().any(|execution| {
            execution.parent.as_ref() == Some(execution_id)
                && declared.contains(&execution.model_node.as_str())
                && execution.state != crate::kernel::ExecutionState::Created
        }) {
            return Ok(None);
        }
        let children = match shape {
            ContainerShape::Branches(branches) => {
                let mut candidates = Vec::with_capacity(branches.len());
                for branch in branches {
                    candidates.push(BranchCandidate {
                        task_id: branch.id.clone(),
                        model_node: node_id(&branch.id)?,
                        predicate: match &branch.r#if {
                            Some(expr) => {
                                self.evaluate_branch_predicate(snapshot, execution_id, expr, inputs)
                                    .await?
                            }
                            None => {
                                if branch.r#else {
                                    PredicateValue::True
                                } else {
                                    PredicateValue::False
                                }
                            }
                        },
                        is_else: branch.r#else && branch.r#if.is_none(),
                    });
                }
                // A predicate that cannot be evaluated fails its branch — the
                // legacy branch task errors in its `init` — instead of wedging
                // the hop. Such a branch is never selected and never decides
                // anything for its siblings; in particular an `else` sibling
                // is blocked: the legacy else runs only when every `if`
                // sibling *skipped*, and an errored one did not.
                let branch_error = |task_id: &str| -> Option<String> {
                    candidates
                        .iter()
                        .find(|candidate| candidate.task_id == task_id)
                        .and_then(|candidate| match &candidate.predicate {
                            PredicateValue::Invalid { message } => Some(message.clone()),
                            _ => None,
                        })
                };
                let has_invalid = candidates
                    .iter()
                    .any(|candidate| branch_error(&candidate.task_id).is_some());
                let decidable: Vec<BranchCandidate> = candidates
                    .iter()
                    .filter(|candidate| {
                        branch_error(&candidate.task_id).is_none()
                            && !(has_invalid && candidate.is_else)
                    })
                    .cloned()
                    .collect();
                let selected: std::collections::HashSet<String> = if decidable.is_empty() {
                    Default::default()
                } else {
                    let command = BranchCommand {
                        process_id: snapshot.process_id.clone(),
                        branch: execution_id.clone(),
                        candidates: decidable,
                        predicate_basis: PredicateBasis::from_snapshot(snapshot),
                        unresolved_dependencies: Vec::new(),
                        source_operation: claim.clone(),
                    };
                    match BranchPlanner::decide(&command) {
                        BranchDecision::Selected(selected) => selected
                            .into_iter()
                            .map(|candidate| candidate.task_id)
                            .collect(),
                        BranchDecision::NoMatch => Default::default(),
                        BranchDecision::Waiting { dependencies } => {
                            return Err(runtime(format!(
                                "branch selection is waiting on unresolved {dependencies:?}"
                            )));
                        }
                        // Every candidate left in the command is evaluable, so
                        // an invalid decision here is a planner inconsistency.
                        BranchDecision::Invalid { message } => return Err(runtime(message)),
                    }
                };
                branches
                    .iter()
                    .map(|branch| {
                        let mut vars: Vec<(String, String)> = branch
                            .vars
                            .iter()
                            .map(|var| (var.name.clone(), var.value.to_string()))
                            .collect();
                        // The failing branch is admitted like a selected one:
                        // its first hop reads the pinned failure and decides
                        // this execution `Failed`, so the parity floor keeps
                        // its terminal execution (the legacy branch task).
                        if let Some(message) = branch_error(&branch.id) {
                            vars.push((
                                BRANCH_ERROR_VAR.to_string(),
                                serde_json::json!({ "code": "", "message": message }).to_string(),
                            ));
                        }
                        Ok(ContainerChild {
                            task_id: branch.id.clone(),
                            model_node: node_id(&branch.id)?,
                            // An unselected branch still becomes one terminal
                            // execution (Skipped) — the parity floor counts a
                            // terminal task per model node, and the legacy
                            // engine keeps the skipped branch task.
                            plan: if selected.contains(&branch.id)
                                || branch_error(&branch.id).is_some()
                            {
                                ContainerChildPlan::Entry
                            } else {
                                ContainerChildPlan::Skip
                            },
                            predecessors: vec![ContainerPredecessor::Source],
                            vars,
                            auto: true,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?
            }
            ContainerShape::Steps(steps) => steps
                .iter()
                .enumerate()
                .map(|(index, step)| {
                    Ok(ContainerChild {
                        task_id: step.id.clone(),
                        model_node: node_id(&step.id)?,
                        plan: if index == 0 {
                            ContainerChildPlan::Entry
                        } else {
                            ContainerChildPlan::Queued
                        },
                        predecessors: if index == 0 {
                            vec![ContainerPredecessor::Source]
                        } else {
                            vec![ContainerPredecessor::Sibling(index - 1)]
                        },
                        vars: step
                            .vars
                            .iter()
                            .map(|var| (var.name.clone(), var.value.to_string()))
                            .collect(),
                        auto: auto_decision(snapshot, &step.id)?,
                    })
                })
                .collect::<Result<Vec<_>>>()?,
            // The step's catch handlers, dispatched because the step ran into
            // its children — not because it failed: their own guards decide
            // what they do without an error to answer (`$ecode()` is null).
            // The failure path fans the same children out with the CATCH sign
            // (see `KernelHost::catch_fanout`).
            ContainerShape::Catches(catches) => {
                let mut children = Vec::with_capacity(catches.len());
                for catch in catches {
                    let mut vars: Vec<(String, String)> = catch
                        .vars
                        .iter()
                        .map(|var| (var.name.clone(), var.value.to_string()))
                        .collect();
                    let mut plan = crate::kernel::ContainerChildPlan::Entry;
                    if let Some(expr) = &catch.r#if {
                        // The handler's own selection: the legacy `Step::init`
                        // evaluates it on the dispatched task and skips the
                        // handler when it does not hold.
                        match self
                            .evaluate_branch_predicate(snapshot, execution_id, expr, inputs)
                            .await?
                        {
                            PredicateValue::True => {}
                            PredicateValue::False => plan = crate::kernel::ContainerChildPlan::Skip,
                            // A handler whose guard cannot be evaluated fails
                            // in its `init` (the legacy `ctx.eval` error): it
                            // is admitted and fails on its first hop.
                            PredicateValue::Null => vars.push((
                                BRANCH_ERROR_VAR.to_string(),
                                serde_json::json!({
                                    "code": "",
                                    "message": format!(
                                        "catch step '{}' guard evaluated to null",
                                        catch.id
                                    )
                                })
                                .to_string(),
                            )),
                            PredicateValue::Invalid { message } => vars.push((
                                BRANCH_ERROR_VAR.to_string(),
                                serde_json::json!({ "code": "", "message": message }).to_string(),
                            )),
                        }
                    }
                    children.push(crate::kernel::ContainerChild {
                        task_id: catch.id.clone(),
                        model_node: node_id(&catch.id)?,
                        plan,
                        predecessors: vec![crate::kernel::ContainerPredecessor::Source],
                        vars,
                        auto: auto_decision(snapshot, &catch.id)?,
                    });
                }
                children
            }
        };
        Ok(Some(ContainerFanoutCommand {
            process_id: snapshot.process_id.clone(),
            source: execution_id.clone(),
            source_operation: claim.clone(),
            children,
            inputs: inputs.to_vec(),
            // A container that fanned out on its own completion parks: the
            // legacy step stays running until its children settled and the
            // join finishes it.
            source_park: crate::kernel::ExecutionState::Waiting,
        }))
    }

    /// Claim → prepare one operation through atomic operation events.
    async fn operation_event(
        &self,
        id: &OperationId,
        expected_revision: u64,
        event: OperationEvent,
    ) -> Result<crate::kernel::OperationRecord> {
        match self
            .aggregates
            .commit_operation_event(id, expected_revision, event)
            .await
            .map_err(runtime)?
        {
            crate::kernel::OperationCommitOutcome::Committed { operation, .. }
            | crate::kernel::OperationCommitOutcome::AlreadyCommitted { operation, .. } => {
                Ok(operation)
            }
            crate::kernel::OperationCommitOutcome::Conflict { .. } => {
                Err(conflict(format!("claim conflict on operation {id:?}")))
            }
        }
    }

    async fn load(&self, process_id: &str) -> Result<Option<crate::kernel::LoadedAggregate>> {
        self.aggregates.load(process_id).await.map_err(runtime)
    }

    pub(crate) async fn snapshot(
        &self,
        process_id: &str,
    ) -> Result<Option<crate::kernel::AggregateSnapshot>> {
        Ok(self.load(process_id).await?.map(|loaded| loaded.snapshot))
    }

    /// Whether the aggregate of `process_id` exists — the durable record of a
    /// pid. A start consults this before anything else so a restart of an
    /// existing run is refused (the legacy duplicate-pid rule), independently
    /// of the projected rows, which a swept run loses while its aggregate
    /// stays.
    pub(crate) async fn exists(&self, process_id: &str) -> Result<bool> {
        Ok(self.load(process_id).await?.is_some())
    }

    /// Whether every execution of the process reached a terminal state.
    pub(crate) async fn process_completed(&self, process_id: &str) -> Result<bool> {
        let Some(snapshot) = self.snapshot(process_id).await? else {
            return Ok(false);
        };
        Ok(snapshot
            .executions
            .values()
            .all(|execution| execution.state.is_terminal()))
    }

    /// Re-derive pending work from the recovery index — the single source of
    /// truth after a crash. Wake and reclaim dispositions re-enter the queue;
    /// committed-but-unfinalized operations are completed; effect-unknown
    /// operations are surfaced for manual reconciliation.
    pub(crate) async fn recover(&self, now_ms: i64) -> Result<RecoveryReport> {
        let mut report = RecoveryReport::default();
        let mut cursor = None;
        let wake = QueueWake {
            tx: self.wake_tx.clone(),
        };
        loop {
            let page = self
                .recovery
                .recoverable_page(cursor.as_ref(), PAGE_SIZE)
                .await
                .map_err(runtime)?;
            if page.operations.is_empty() {
                break;
            }
            for operation in page.operations {
                report.scanned += 1;
                match RecoveryPlanner::classify(&operation, now_ms) {
                    RecoveryDisposition::Wake => {
                        wake.wake(&operation.id);
                        report.woken.push(operation.id);
                    }
                    RecoveryDisposition::AwaitOwner => {
                        report.awaiting_owner.push(operation.id);
                    }
                    RecoveryDisposition::ReclaimExpired => {
                        match self
                            .operation_event(
                                &operation.id,
                                operation.revision,
                                OperationEvent::ReleaseExpiredClaim { now_ms },
                            )
                            .await
                        {
                            Ok(reclaimed) => match reclaimed.state {
                                OperationState::RetryScheduled { .. } => {
                                    wake.wake(&reclaimed.id);
                                    report.woken.push(reclaimed.id);
                                }
                                OperationState::EffectUnknown { .. } => {
                                    report.reconciliation_required.push(reclaimed.id);
                                }
                                _ => {}
                            },
                            Err(error) => {
                                #[cfg(test)]
                                eprintln!("reclaim {operation:?} failed: {error}");
                                tracing::debug!(operation = ?operation.id, error = %error, "reclaim lost; next scan retries");
                            }
                        }
                    }
                    RecoveryDisposition::FinalizeCommit => {
                        let commit = match &operation.state {
                            OperationState::Committed(commit) => commit.clone(),
                            _ => continue,
                        };
                        match self
                            .operation_event(
                                &operation.id,
                                operation.revision,
                                OperationEvent::Complete { commit },
                            )
                            .await
                        {
                            Ok(finalized) => report.finalized.push(finalized.id),
                            Err(error) => {
                                tracing::debug!(operation = ?operation.id, error = %error, "finalize lost; next scan retries");
                            }
                        }
                    }
                    RecoveryDisposition::ManualReconciliation => {
                        report.reconciliation_required.push(operation.id);
                    }
                    RecoveryDisposition::IgnoreTerminal => {}
                }
            }
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        Ok(report)
    }

    /// One outbox delivery pass over the attached transport: claim every due
    /// record with a fenced claim, hand it to the transport, then persist the
    /// reported outcome. After the transport ran, only its explicit `RetryAt`
    /// proves no external effect — everything else the transport cannot
    /// classify stays `DeliveryUnknown`. The transport lock is taken only for
    /// the synchronous `deliver` call, never across an await.
    pub(crate) async fn dispatch_outbox(&self) -> Result<usize> {
        let now = SystemClock.now_ms();
        let mut delivered = 0usize;
        let mut settled: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut cursor = None;
        loop {
            let page = self
                .aggregates
                .dispatchable_outbox_page(now, cursor.as_ref(), PAGE_SIZE)
                .await
                .map_err(runtime)?;
            if page.records.is_empty() {
                break;
            }
            for record in page.records {
                let claim = DeliveryClaim {
                    owner: OWNER.into(),
                    fence: record.last_fence + 1,
                    lease_until_ms: now + DELIVERY_LEASE_MS,
                    heartbeat_revision: 0,
                };
                let claimed = match self
                    .aggregates
                    .evolve_outbox(
                        &record.id,
                        record.revision,
                        OutboxEvent::Claim {
                            claim: claim.clone(),
                            now_ms: now,
                        },
                    )
                    .await
                {
                    Ok(claimed) => claimed,
                    Err(error) => {
                        // A concurrent worker claimed it first; skip without
                        // failing the pass.
                        tracing::debug!(record = ?record.id, error = %error, "delivery claim lost");
                        continue;
                    }
                };
                let event = {
                    let outcome = self.transport.write().deliver(&claimed);
                    match outcome {
                        crate::kernel::DeliveryOutcome::Delivered { receipt } => {
                            delivered += 1;
                            settled.insert(claimed.id.process_id.clone());
                            OutboxEvent::ConfirmDelivered {
                                claim: claim.clone(),
                                receipt,
                                now_ms: now,
                            }
                        }
                        crate::kernel::DeliveryOutcome::RetryAt {
                            available_at_ms,
                            error,
                        } => OutboxEvent::Reschedule {
                            claim: claim.clone(),
                            available_at_ms,
                            error,
                            now_ms: now,
                        },
                        crate::kernel::DeliveryOutcome::OutcomeUnknown { reason } => {
                            OutboxEvent::MarkDeliveryUnknown {
                                claim: claim.clone(),
                                reason,
                                now_ms: now,
                            }
                        }
                        crate::kernel::DeliveryOutcome::Quarantine { reason } => {
                            OutboxEvent::Quarantine { reason }
                        }
                    }
                };
                self.aggregates
                    .evolve_outbox(&record.id, claimed.revision, event)
                    .await
                    .map_err(runtime)?;
            }
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        // The deliveries that settled decide the read model's `removable`
        // flag: a run that finished before its records were delivered only
        // hands its rows (and directory) to the sweeper once they are.
        for process_id in &settled {
            self.refresh_proc_row(process_id).await;
        }
        Ok(delivered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::{ActionKind, DeliveryOutcome, OutboxRecord, OutboxState};

    struct CountingTransport {
        delivered: Arc<parking_lot::Mutex<usize>>,
    }

    impl crate::kernel::OutboxTransport for CountingTransport {
        fn deliver(&mut self, _record: &OutboxRecord) -> DeliveryOutcome {
            *self.delivered.lock() += 1;
            DeliveryOutcome::Delivered {
                receipt: "ok".into(),
            }
        }
    }

    async fn assert_all_terminal(host: &KernelHost, pid: &str) {
        let snapshot = host.snapshot(pid).await.unwrap().expect("aggregate exists");
        assert!(!snapshot.executions.is_empty());
        for (id, execution) in &snapshot.executions {
            assert!(
                execution.state.is_terminal(),
                "execution {id:?} stuck at {:?}",
                execution.state
            );
        }
        for (id, record) in &snapshot.operations {
            assert!(
                matches!(record.state, OperationState::Completed(_)),
                "operation {id:?} stuck at {:?}",
                record.state
            );
        }
    }

    #[tokio::test]
    async fn linear_process_runs_to_completion_and_delivers_outbox() {
        let (host, mut wake_rx) = KernelHost::in_memory();
        let spec = ProcessSpec::linear("p1", "root", &["a", "b"]);
        host.start_process(&spec).await.unwrap();
        let report = host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
        // 3 nodes x (execute + action + propagate) hops, root included.
        assert_eq!(report.executed, 9, "nine hops expected: {report:?}");
        assert_eq!(report.retried, 0);
        assert_all_terminal(&host, "p1").await;

        let snapshot = host.snapshot("p1").await.unwrap().unwrap();
        // started x3 + completed x3 + propagated x3.
        assert_eq!(snapshot.outbox.len(), 9);

        let counter = Arc::new(parking_lot::Mutex::new(0));
        host.attach_outbox_transport(Box::new(CountingTransport {
            delivered: counter.clone(),
        }));
        let delivered = host.dispatch_outbox().await.unwrap();
        assert_eq!(delivered, 9);
        assert_eq!(*counter.lock(), 9);
        let snapshot = host.snapshot("p1").await.unwrap().unwrap();
        assert!(
            snapshot
                .outbox
                .values()
                .all(|record| matches!(record.state, OutboxState::Delivered { .. }))
        );
    }

    #[tokio::test]
    async fn wake_loss_is_recovered_by_the_scan() {
        let (host, mut wake_rx) = KernelHost::in_memory();
        let spec = ProcessSpec::linear("p2", "root", &["only"]);
        // Create without waking: everything now rides on the recovery scan,
        // exactly like after a crash that lost the in-memory queue.
        let snapshot = spec.seed_snapshot().unwrap();
        host.aggregates
            .create(&snapshot)
            .await
            .map_err(runtime)
            .unwrap();
        let now = SystemClock.now_ms();
        let report = host.recover(now).await.unwrap();
        assert!(
            !report.woken.is_empty(),
            "the pending root operation must be re-derived: {report:?}"
        );
        host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
        assert!(host.process_completed("p2").await.unwrap());
    }

    /// The parity floor: the same flat workflow driven by the legacy engine
    /// and by the kernel host must reach the same terminal task set. Every
    /// P2 cutover slice extends this harness instead of asserting in isolation.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn kernel_path_matches_legacy_for_flat_chain() {
        let workflow = {
            let mut workflow = crate::model::Workflow::new().with_id("wf-parity");
            workflow = workflow.with_step(|step| step.with_id("s1"));
            workflow = workflow.with_step(|step| step.with_id("s2"));
            workflow
        };
        let expected_tasks: Vec<String> = vec![
            crate::utils::consts::TASK_ROOT_TID.to_string(),
            "s1".into(),
            "s2".into(),
        ];

        // Legacy engine: start and wait for the process to settle.
        let engine = crate::Engine::builder().start().await.unwrap();
        let sig = engine.signal(());
        let done = sig.clone();
        engine.channel().on_complete(move |_| {
            let done = done.clone();
            async move { done.close() }
        });
        let legacy_proc = engine
            .runtime()
            .start(&workflow, crate::Vars::new())
            .await
            .unwrap();
        let pid = legacy_proc.id().to_string();
        tokio::time::timeout(std::time::Duration::from_secs(30), sig.recv())
            .await
            .expect("legacy engine never finished");
        let legacy = engine.runtime().proc(&pid).await.unwrap().unwrap();
        let mut legacy_tasks: Vec<String> =
            legacy.tasks().iter().map(|task| task.id.clone()).collect();
        legacy_tasks.sort();

        // Kernel host: same model, same expectation.
        let (host, mut wake_rx) = KernelHost::in_memory();
        let spec = ProcessSpec::from_workflow("k-parity", &workflow, &|_| None).unwrap();
        host.start_process(&spec).await.unwrap();
        host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
        assert!(host.process_completed("k-parity").await.unwrap());
        let snapshot = host.snapshot("k-parity").await.unwrap().unwrap();
        let kernel_tasks: Vec<String> = snapshot
            .executions
            .keys()
            .map(|id| id.task_id.clone())
            .collect();

        // The kernel derives execution identity from model node ids; the
        // legacy engine mints random task ids for the acts it runs. Parity
        // here is the terminal task set size and completion, not id equality.
        assert_eq!(legacy_tasks.len(), kernel_tasks.len());
        assert_eq!(legacy_tasks.len(), expected_tasks.len());
        assert!(snapshot.executions.values().all(|e| e.state.is_terminal()));
    }

    /// The driver switch: with `kernel_engine` on the kernel host *is* the
    /// engine. A start seeds the aggregate and no legacy process runs, yet the
    /// engine's own completion event still fires — the host reports the run's
    /// lifecycle — and every execution settles.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn kernel_engine_serves_a_run_to_completion() {
        let mut config = crate::Config::default();
        config.data.kernel_engine = true;
        config.data.tick_interval_secs = Some(1);
        let engine = crate::Engine::builder()
            .set_config(&config)
            .start()
            .await
            .unwrap();
        let sig = engine.signal(());
        let done = sig.clone();
        engine.channel().on_complete(move |_| {
            let done = done.clone();
            async move { done.close() }
        });

        let workflow = {
            let mut workflow = crate::model::Workflow::new().with_id("wf-driver");
            workflow = workflow.with_step(|step| step.with_id("s1"));
            workflow = workflow.with_step(|step| step.with_id("s2"));
            workflow
        };
        let proc = engine
            .runtime()
            .start(&workflow, crate::Vars::new())
            .await
            .unwrap();
        let pid = proc.id().to_string();
        tokio::time::timeout(std::time::Duration::from_secs(30), sig.recv())
            .await
            .expect("engine never finished");

        // No legacy scheduler was launched: the run's durable record is the
        // aggregate, and the legacy rows are its projection (the read model
        // this driver serves).
        let runtime = engine.runtime();
        assert!(
            runtime.cache().resident(&pid).is_none(),
            "driver mode must not hold a resident legacy process"
        );
        let host = runtime.kernel().expect("the kernel host must be wired");
        assert!(host.process_completed(&pid).await.unwrap());
        let snapshot = host.snapshot(&pid).await.unwrap().expect("aggregate");
        let mut tasks: Vec<String> = snapshot
            .executions
            .keys()
            .map(|id| id.task_id.clone())
            .collect();
        tasks.sort();
        assert_eq!(
            tasks,
            vec![
                crate::utils::consts::TASK_ROOT_TID.to_string(),
                "s1".into(),
                "s2".into()
            ]
        );
        assert!(
            runtime.proc(&pid).await.unwrap().is_some(),
            "the projected rows serve the read API"
        );
    }

    /// The driver switch, action half: a waiting (Irq) act's Created message
    /// comes from the kernel's outbox through the engine channel, and the
    /// client's `act:complete` is served by the host — no legacy engine runs
    /// on either side.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn kernel_engine_serves_client_actions_on_an_irq_workflow() {
        use crate::event::EventAction;

        let mut config = crate::Config::default();
        config.data.kernel_engine = true;
        config.data.tick_interval_secs = Some(1);
        let engine = crate::Engine::builder()
            .set_config(&config)
            .start()
            .await
            .unwrap();
        let workflow = {
            let mut workflow = crate::model::Workflow::new().with_id("wf-irq");
            workflow = workflow.with_step(|step| {
                step.with_id("s1").with_uses(
                    crate::utils::test::USES_IRQ,
                    crate::Vars::new().with("key", "act1"),
                )
            });
            workflow = workflow.with_step(|step| {
                step.with_id("s2").with_uses(
                    crate::utils::test::USES_IRQ,
                    crate::Vars::new().with("key", "act2"),
                )
            });
            workflow
        };
        let runtime = engine.runtime();
        let host = runtime.kernel().expect("the kernel host must be wired");

        // Subscribers attach BEFORE the start: the IRQ act's Created message
        // fires the moment the act runs, and a late subscriber misses it.
        let wait_for_created = |engine: &crate::Engine, key: &'static str| {
            let sig = engine.signal((String::new(), String::new()));
            let (done, done_rx) = (sig.clone(), sig.clone());
            let channel = engine.channel();
            channel.on_message(move |e| {
                let done = done_rx.clone();
                let key = key.to_string();
                async move {
                    if e.is_params_key(&key) && e.is_state(crate::MessageState::Created) {
                        done.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                        done.close();
                    }
                }
            });
            done
        };
        let created1 = wait_for_created(&engine, "act1");
        let created2 = wait_for_created(&engine, "act2");

        let proc = runtime.start(&workflow, crate::Vars::new()).await.unwrap();
        let pid = proc.id().to_string();

        // Each waiting act completes exactly when its client action arrives.
        for created in [created1, created2] {
            let (pid, tid) =
                tokio::time::timeout(std::time::Duration::from_secs(30), created.recv())
                    .await
                    .expect("waiting act never created")
                    .clone();
            runtime
                .do_action(&crate::event::Action::new(
                    &pid,
                    &tid,
                    EventAction::Next,
                    crate::Vars::new(),
                ))
                .await
                .unwrap_or_else(|err| panic!("the kernel must serve act:complete: {err}"));
        }

        // The run settles once both acts are completed.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            if host.process_completed(&pid).await.unwrap_or(false) {
                break;
            }
            let states = host.snapshot(&pid).await.unwrap().map(|snapshot| {
                snapshot
                    .executions
                    .iter()
                    .map(|(id, execution)| format!("{}:{:?}", id.task_id, execution.state))
                    .collect::<Vec<_>>()
            });
            assert!(
                std::time::Instant::now() < deadline,
                "the run never settled: {states:?}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    /// Driver mode, IO half: the effect's headless scope names the run's own
    /// directory, so a filesystem package reads `$env.WORK_DIR` (and
    /// `Context::workdir`) as `<workdir>/<pid>` — the same directory the
    /// legacy start made.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn kernel_engine_scopes_effects_to_the_run_workdir() {
        let root = std::env::temp_dir().join(format!("acts-kernel-wd-{}", crate::utils::longid()));
        let mut config = crate::Config::default();
        config.data.kernel_engine = true;
        config.data.tick_interval_secs = Some(1);
        config.data.workdir = Some(root.display().to_string());
        let engine = crate::Engine::builder()
            .set_config(&config)
            .start()
            .await
            .unwrap();
        let sig = engine.signal(());
        let done = sig.clone();
        engine.channel().on_complete(move |_| {
            let done = done.clone();
            async move { done.close() }
        });

        let workflow = crate::model::Workflow::new()
            .with_id("wf-workdir")
            .with_step(|step| {
                step.with_id("s1").with_uses(
                    crate::utils::test::USES_SET,
                    crate::Vars::new().with("dir", "${{ $env.WORK_DIR }}"),
                )
            });
        let proc = engine
            .runtime()
            .start(&workflow, crate::Vars::new())
            .await
            .unwrap();
        let pid = proc.id().to_string();
        tokio::time::timeout(std::time::Duration::from_secs(30), sig.recv())
            .await
            .expect("engine never finished");

        let host = engine.runtime().kernel().expect("host").clone();
        let snapshot = host.snapshot(&pid).await.unwrap().unwrap();
        let s1 = crate::kernel::ExecutionId::new("s1", 0).unwrap();
        let dir = snapshot
            .variables
            .get(&(s1, "dir".to_string()))
            .expect("the effect output")
            .clone();
        assert_eq!(
            serde_json::from_str::<String>(&dir).unwrap(),
            root.join(&pid).display().to_string()
        );

        std::fs::remove_dir_all(&root).ok();
        engine.close().await;
    }

    /// The read model in driver mode: the kernel's rows land in the legacy
    /// collections, so `proc:get`/`task:get`/`Runtime::proc` answer from them,
    /// and once the run settled the sweeper reclaims its rows and its
    /// directory.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn kernel_engine_projects_the_legacy_read_model() {
        let root =
            std::env::temp_dir().join(format!("acts-kernel-rows-{}", crate::utils::longid()));
        let mut config = crate::Config::default();
        config.data.kernel_engine = true;
        config.data.tick_interval_secs = Some(1);
        config.data.workdir = Some(root.display().to_string());
        let engine = crate::Engine::builder()
            .set_config(&config)
            .start()
            .await
            .unwrap();
        let sig = engine.signal(());
        let done = sig.clone();
        // An ack channel: its delivery rows only persist when the process's row
        // exists, so they are a direct probe of the projection (and of the
        // delivery close that lets a finished run be swept).
        let channel = engine.channel_with_options(&crate::ChannelOptions {
            ack: true,
            ..Default::default()
        });
        channel.on_complete(move |_| {
            let done = done.clone();
            async move { done.close() }
        });
        // The act messages only reach an ack channel that subscribes to them.
        channel.on_message(|_| async move {});

        let workflow = crate::model::Workflow::new()
            .with_id("wf-rows")
            .with_step(|step| {
                step.with_id("s1").with_uses(
                    crate::utils::test::USES_SET,
                    crate::Vars::new().with("k1", "v1"),
                )
            });
        let runtime = engine.runtime();
        let proc = runtime.start(&workflow, crate::Vars::new()).await.unwrap();
        let pid = proc.id().to_string();
        tokio::time::timeout(std::time::Duration::from_secs(30), sig.recv())
            .await
            .expect("the run never finished");

        let executor = engine.executor(&crate::Principal::unrestricted());
        let info = executor.proc().get(&pid).await.expect("the proc row");
        assert_eq!(info.state, "completed");
        assert_eq!(info.mid, "wf-rows");
        let mut tasks: Vec<String> = info.tasks.iter().map(|task| task.id.clone()).collect();
        tasks.sort();
        assert_eq!(
            tasks,
            vec![crate::utils::consts::TASK_ROOT_TID.to_string(), "s1".into()]
        );
        // The effect's output is the task scope's projected data.
        let s1 = executor.task().get(&pid, "s1").await.expect("the task row");
        assert_eq!(s1.state, "completed");
        assert_eq!(s1.r#type, "step");
        assert!(
            s1.data.contains("\"k1\":\"v1\""),
            "the task data must carry the effect output: {}",
            s1.data
        );
        // `Runtime::proc` rebuilds the process from the rows: the durable read
        // model, not a resident instance.
        let live = runtime
            .proc(&pid)
            .await
            .unwrap()
            .expect("the projected process must load");
        assert_eq!(live.tasks().len(), 2);

        // The run's own directory is what the effect used; it goes with the
        // rows once the deliveries settled (the outbox pass projects that) and
        // the sweeper runs.
        let dir = root.join(&pid);
        assert!(dir.is_dir(), "the run's directory must exist");
        let host = runtime.kernel().expect("the kernel host").clone();
        host.dispatch_outbox().await.unwrap();
        // The act message reached the engine's durable message rows (the ack
        // channel's delivery could only be stored once the process's row
        // existed), and the finished task settled it…
        let messages = executor
            .msg()
            .list(
                &crate::store::query::Query::new().filter(
                    crate::store::query::Filter::and()
                        .expr(crate::store::query::Expr::eq("pid", pid.clone())),
                ),
            )
            .await
            .expect("the message rows");
        let mut act_states: Vec<String> = messages
            .rows
            .iter()
            .filter(|row| row.tid == "s1")
            .map(|row| row.state.to_string())
            .collect();
        act_states.sort();
        assert_eq!(
            act_states,
            vec!["completed".to_string(), "created".to_string()],
            "the act's Created and Completed messages must both have rows: {:?}",
            messages.rows
        );
        // The finished task settled every delivery of its messages, so the
        // run is sweepable.
        for row in &messages.rows {
            assert_eq!(
                row.status, "completed",
                "every delivery of a finished run must be settled: {:?}",
                messages.rows
            );
        }
        runtime.cache().sweep_removable().await.unwrap();
        assert!(!dir.exists(), "the sweeper reclaims the run's directory");
        assert!(
            runtime
                .cache()
                .store()
                .procs()
                .find_opt(&pid)
                .await
                .unwrap()
                .is_none()
        );
        assert!(runtime.proc(&pid).await.unwrap().is_none());

        std::fs::remove_dir_all(&root).ok();
        engine.close().await;
    }

    /// Restart discrimination: a pid's durable record is the aggregate, so a
    /// same-pid start is refused both while the projected rows exist and after
    /// the sweeper reclaimed them — the aggregate outlives the rows.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn kernel_engine_refuses_a_started_pid_via_the_aggregate() {
        let mut config = crate::Config::default();
        config.data.kernel_engine = true;
        config.data.tick_interval_secs = Some(1);
        let engine = crate::Engine::builder()
            .set_config(&config)
            .start()
            .await
            .unwrap();
        let sig = engine.signal(());
        let done = sig.clone();
        engine.channel().on_complete(move |_| {
            let done = done.clone();
            async move { done.close() }
        });

        let workflow = crate::model::Workflow::new()
            .with_id("wf-dup")
            .with_step(|step| step.with_id("s1"));
        let runtime = engine.runtime();
        let pid = "duppid";
        let options = crate::Vars::new().with(crate::utils::consts::PROCESS_ID, pid);
        runtime.start(&workflow, options.clone()).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(30), sig.recv())
            .await
            .expect("the run never finished");

        let duplicated = runtime.start(&workflow, options.clone()).await;
        assert!(
            duplicated.is_err_and(|err| err.to_string().contains("duplicated")),
            "an existing pid must be refused while its rows are there"
        );

        // Settle and sweep the rows: the aggregate is what still answers.
        let host = runtime.kernel().expect("the kernel host").clone();
        host.dispatch_outbox().await.unwrap();
        runtime.cache().sweep_removable().await.unwrap();
        assert!(
            runtime
                .cache()
                .store()
                .procs()
                .find_opt(pid)
                .await
                .unwrap()
                .is_none(),
            "the settled run's rows must be swept"
        );
        assert!(host.exists(pid).await.unwrap(), "the aggregate stays");
        let duplicated = runtime.start(&workflow, options).await;
        assert!(
            duplicated.is_err_and(|err| err.to_string().contains("duplicated")),
            "the aggregate must refuse a restart the swept rows no longer show"
        );

        engine.close().await;
    }

    /// The package write-back: what a Func package leaves in its context —
    /// the returned outputs, the task data it wrote, and the process env it
    /// set — commits with the completion, the env on the run's root scope,
    /// where a later task's `$env` reads it and the projected proc row carries
    /// it.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn effect_write_back_folds_into_the_aggregate() {
        struct WriteBack;
        #[async_trait::async_trait]
        impl crate::package::ActPackage for WriteBack {
            fn new(_: &crate::Config) -> crate::Result<Self> {
                Ok(Self)
            }
            fn definition() -> crate::package::ActPackageDefinition {
                crate::package::ActPackageDefinition {
                    id: "test.writeback",
                    name: "WriteBack",
                    desc: "",
                    icon: "",
                    doc: "",
                    version: "0.1.0",
                    schema: serde_json::json!({}),
                    options: None,
                    run_as: crate::package::ActRunAs::Func,
                    resources: Vec::new(),
                    catalog: crate::package::ActPackageCatalog::App,
                }
            }
            async fn execute(
                &self,
                ctx: &crate::Context,
                _params: &serde_json::Value,
            ) -> crate::Result<Option<crate::Vars>> {
                ctx.set_env("GREETING", "hi");
                ctx.task()
                    .update_data(&crate::Vars::new().with("written", 7));
                Ok(Some(crate::Vars::new().with("output", 1)))
            }
        }
        struct ReadEnv;
        #[async_trait::async_trait]
        impl crate::package::ActPackage for ReadEnv {
            fn new(_: &crate::Config) -> crate::Result<Self> {
                Ok(Self)
            }
            fn definition() -> crate::package::ActPackageDefinition {
                crate::package::ActPackageDefinition {
                    id: "test.readenv",
                    name: "ReadEnv",
                    desc: "",
                    icon: "",
                    doc: "",
                    version: "0.1.0",
                    schema: serde_json::json!({}),
                    options: None,
                    run_as: crate::package::ActRunAs::Func,
                    resources: Vec::new(),
                    catalog: crate::package::ActPackageCatalog::App,
                }
            }
            async fn execute(
                &self,
                ctx: &crate::Context,
                _params: &serde_json::Value,
            ) -> crate::Result<Option<crate::Vars>> {
                Ok(Some(
                    crate::Vars::new().with("seen", ctx.get_env::<String>("GREETING")),
                ))
            }
        }

        let mut config = crate::Config::default();
        config.data.kernel_engine = true;
        config.data.tick_interval_secs = Some(1);
        let engine = crate::Engine::builder()
            .set_config(&config)
            .start()
            .await
            .unwrap();
        let runtime = engine.runtime();
        let package = runtime.package().clone();
        // The production path: the durable metadata row (what the effect
        // runner resolves `run_as`/params against) plus the instance factory.
        let executor = engine.executor(&crate::Principal::unrestricted());
        executor
            .ext()
            .register_package(&<WriteBack as crate::package::ActPackage>::definition())
            .await
            .unwrap();
        executor
            .ext()
            .register_package(&<ReadEnv as crate::package::ActPackage>::definition())
            .await
            .unwrap();
        package.register(
            "test.writeback",
            &crate::package::ActPackageRegister::new::<WriteBack>(),
        );
        package.register(
            "test.readenv",
            &crate::package::ActPackageRegister::new::<ReadEnv>(),
        );
        let sig = engine.signal(());
        let done = sig.clone();
        engine.channel().on_complete(move |_| {
            let done = done.clone();
            async move { done.close() }
        });

        let workflow = crate::model::Workflow::new()
            .with_id("wf-writeback")
            .with_step(|step| {
                step.with_id("s1")
                    .with_uses("test.writeback", crate::Vars::new())
            })
            .with_step(|step| {
                step.with_id("s2")
                    .with_uses("test.readenv", crate::Vars::new())
            });
        let proc = runtime.start(&workflow, crate::Vars::new()).await.unwrap();
        let pid = proc.id().to_string();
        tokio::time::timeout(std::time::Duration::from_secs(30), sig.recv())
            .await
            .expect("the run never finished");

        let host = runtime.kernel().expect("the kernel host").clone();
        let snapshot = host.snapshot(&pid).await.unwrap().unwrap();
        let var = |task: &str, name: &str| {
            snapshot
                .variables
                .get(&(
                    crate::kernel::ExecutionId::new(task, 0).unwrap(),
                    name.to_string(),
                ))
                .cloned()
        };
        assert_eq!(var("s1", "output").as_deref(), Some("1"));
        assert_eq!(var("s1", "written").as_deref(), Some("7"));
        // The env the package set is the run's, read back by the next task.
        assert_eq!(var("s2", "seen").as_deref(), Some("\"hi\""));
        // …and it is projected into the run's row, exactly like the legacy
        // process env.
        let row = runtime
            .cache()
            .store()
            .procs()
            .find(&pid)
            .await
            .expect("the proc row");
        assert!(
            row.env.contains("\"GREETING\":\"hi\""),
            "the proc env must carry the package env: {}",
            row.env
        );

        engine.close().await;
    }

    /// The act message stream of one workflow, on both engines: the legacy
    /// engine's per-task messages folded onto their owning node (an act folds
    /// into its step; the workflow root is `$`) and the kernel driver's
    /// forwarded messages must agree per (node, state, uses).
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn kernel_act_messages_match_the_legacy_run() {
        use crate::event::EventAction;

        // One run, ONE engine flavour, per call: both sides speak the same
        // engine-channel API, so the collection and the act completion are
        // shared. Returns (folded act messages, lifecycle events seen, folded
        // read-model rows) — the lifecycle is the process-level channel
        // (`on_start`/`on_complete`), which is where the kernel reports the
        // workflow root the legacy engine also announced as a task message,
        // and the rows are what `proc:get`/`task:get` read, folded the same
        // way on both engines (an act row folds to its owning step's node;
        // only its states are compared).
        async fn run(kernel_engine: bool) -> (Vec<String>, bool, bool, Vec<String>) {
            let workflow = crate::model::Workflow::new()
                .with_id("wf-msg-parity")
                .with_step(|step| {
                    step.with_id("s1").with_uses(
                        crate::utils::test::USES_SET,
                        crate::Vars::new().with("k1", "v1"),
                    )
                })
                .with_step(|step| {
                    step.with_id("s2").with_uses(
                        crate::utils::test::USES_IRQ,
                        crate::Vars::new().with("key", "irq-1"),
                    )
                });
            let mut config = crate::Config::default();
            config.data.kernel_engine = kernel_engine;
            config.data.tick_interval_secs = Some(1);
            let engine = crate::Engine::builder()
                .set_config(&config)
                .start()
                .await
                .unwrap();
            let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let sig = engine.signal(());
            let done = sig.clone();
            {
                let completed = completed.clone();
                engine.channel().on_complete(move |_| {
                    let completed = completed.clone();
                    let done = done.clone();
                    async move {
                        completed.store(true, std::sync::atomic::Ordering::SeqCst);
                        done.close();
                    }
                });
            }
            {
                let started = started.clone();
                engine.channel().on_start(move |_| {
                    let started = started.clone();
                    async move {
                        started.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                });
            }
            let folded: Arc<parking_lot::Mutex<Vec<String>>> =
                Arc::new(parking_lot::Mutex::new(Vec::new()));
            let runtime = engine.runtime();
            {
                let folded = folded.clone();
                engine.channel().on_message(move |e| {
                    let folded = folded.clone();
                    async move {
                        let uses = e.uses.clone().unwrap_or_default();
                        // The workflow root's task messages are the process
                        // lifecycle, which the kernel reports on the lifecycle
                        // channel (asserted below) — not act messages.
                        if e.r#type == "workflow" {
                            return;
                        }
                        // Fold onto the owning node: an act belongs to its step
                        // (the kernel's node identity), everything else is its
                        // own node.
                        let node = if e.r#type == "act" {
                            e.inputs
                                .get::<crate::Vars>(crate::utils::consts::STEP_KEY)
                                .and_then(|step| {
                                    step.get::<String>(crate::utils::consts::STEP_NODE_ID)
                                })
                                .unwrap_or_else(|| e.nid.clone())
                        } else {
                            e.nid.clone()
                        };
                        let state = e.state.to_string();
                        folded
                            .lock()
                            .push(format!("{node}|{state}|{}", !uses.is_empty()));
                    }
                });
            }
            let created = engine.signal((String::new(), String::new()));
            {
                let created = created.clone();
                engine.channel().on_message(move |e| {
                    let created = created.clone();
                    async move {
                        if e.r#type == "act"
                            && e.is_params_key("irq-1")
                            && e.state == crate::MessageState::Created
                        {
                            created.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                            created.close();
                        }
                    }
                });
            }

            let proc = runtime.start(&workflow, crate::Vars::new()).await.unwrap();
            let _ = proc.id();
            // The client's action completes the waiting act, on the tid its
            // own Created message named (the legacy act's random task id, the
            // kernel's node id).
            let (pid, tid) =
                tokio::time::timeout(std::time::Duration::from_secs(30), created.recv())
                    .await
                    .expect("the waiting act was never created")
                    .clone();
            runtime
                .do_action(&crate::event::Action::new(
                    &pid,
                    &tid,
                    EventAction::Next,
                    crate::Vars::new(),
                ))
                .await
                .expect("the waiting act must complete");
            tokio::time::timeout(std::time::Duration::from_secs(30), sig.recv())
                .await
                .expect("the run never finished");
            // The driver delivers records on its tick; drain them so the
            // comparison sees the same settled stream the legacy engine
            // emitted synchronously.
            if let Some(host) = runtime.kernel() {
                host.dispatch_outbox().await.unwrap();
            }
            // The durable read model of this run, folded: a row's node is its
            // own `node_data` id (the workflow root is `$`, as in the kernel),
            // and an act row folds to the node of its parent row. The legacy
            // writer is drained first: its rows lag the completion signal.
            runtime.cache().flush().await.unwrap();
            let rows_raw = runtime
                .cache()
                .store()
                .tasks()
                .query_all(
                    &crate::store::query::Query::new().filter(
                        crate::store::query::Filter::and()
                            .expr(crate::store::query::Expr::eq("pid", pid.clone())),
                    ),
                )
                .await
                .unwrap();
            let node_id_of = |tid: &str| {
                rows_raw
                    .iter()
                    .find(|row| row.tid == tid)
                    .and_then(|row| {
                        serde_json::from_str::<crate::scheduler::NodeData>(&row.node_data).ok()
                    })
                    .map(|data| data.id)
            };
            let mut rows: Vec<String> = Vec::new();
            for row in &rows_raw {
                let node = if row.kind == "workflow" {
                    crate::utils::consts::TASK_ROOT_TID.to_string()
                } else if row.kind == "act" {
                    row.parent
                        .as_deref()
                        .and_then(node_id_of)
                        .unwrap_or_else(|| row.tid.clone())
                } else {
                    node_id_of(&row.tid).unwrap_or_else(|| row.tid.clone())
                };
                rows.push(format!("{node}|{}", row.state));
            }
            rows.sort();
            rows.dedup();
            let mut folded = folded.lock().clone();
            folded.sort();
            folded.dedup();
            engine.close().await;
            (
                folded,
                started.load(std::sync::atomic::Ordering::SeqCst),
                completed.load(std::sync::atomic::Ordering::SeqCst),
                rows,
            )
        }

        let (legacy, legacy_started, legacy_completed, legacy_rows) = run(false).await;
        let (kernel, kernel_started, kernel_completed, kernel_rows) = run(true).await;
        assert_eq!(legacy, kernel, "the folded act message sets must match");
        assert_eq!(
            legacy_rows, kernel_rows,
            "the folded read-model rows must match"
        );
        // The workflow root's messages are the process lifecycle on both
        // engines: one start and one completion.
        assert!(legacy_started && legacy_completed);
        assert_eq!(kernel_started, legacy_started);
        assert_eq!(kernel_completed, legacy_completed);
    }

    /// A client's `act:error` carries the legacy inputs (`ecode`, `message`):
    /// the failure must land on the same node with the same code on both
    /// engines — what `$ecode()` reads for a catch is this code.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn kernel_error_action_carries_the_legacy_inputs() {
        use crate::event::EventAction;

        /// Drive one engine: start, error the waiting act with `ecode`, then
        /// hand back the step's durable error.
        async fn run_error(kernel_engine: bool) -> Option<String> {
            let workflow = crate::model::Workflow::new()
                .with_id("wf-error-action")
                .with_step(|step| {
                    step.with_id("s1").with_uses(
                        crate::utils::test::USES_IRQ,
                        crate::Vars::new().with("key", "err-act"),
                    )
                });
            let mut config = crate::Config::default();
            config.data.kernel_engine = kernel_engine;
            config.data.tick_interval_secs = Some(1);
            let engine = crate::Engine::builder()
                .set_config(&config)
                .start()
                .await
                .unwrap();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(String, String)>();
            engine.channel().on_message(move |e| {
                let tx = tx.clone();
                async move {
                    if e.r#type == "act"
                        && e.is_params_key("err-act")
                        && e.state == crate::MessageState::Created
                    {
                        let _ = tx.send((e.pid.clone(), e.tid.clone()));
                    }
                }
            });
            let runtime = engine.runtime();
            runtime
                .start(
                    &workflow,
                    crate::Vars::new().with(crate::utils::consts::PROCESS_ID, "p_err_action"),
                )
                .await
                .unwrap();
            let (pid, tid) = tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv())
                .await
                .expect("the act was never created")
                .expect("the event channel closed");
            runtime
                .do_action(&crate::event::Action::new(
                    &pid,
                    &tid,
                    EventAction::Error,
                    crate::Vars::new()
                        .with(crate::utils::consts::ACT_ERR_CODE, "boom")
                        .with(crate::utils::consts::ACT_ERR_MESSAGE, "boom-msg"),
                ))
                .await
                .expect("the act must error");
            // Wait for the walk to settle, then read the step's durable error.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                runtime.cache().flush().await.unwrap();
                let rows = runtime
                    .cache()
                    .store()
                    .tasks()
                    .query_all(&crate::store::query::Query::new().filter(
                        crate::store::query::Filter::and().expr(crate::store::query::Expr::eq(
                            "pid",
                            "p_err_action".to_string(),
                        )),
                    ))
                    .await
                    .unwrap();
                let step_error = rows.iter().find_map(|row| {
                    let node: crate::scheduler::NodeData =
                        serde_json::from_str(&row.node_data).ok()?;
                    (node.id == "s1").then(|| row.err.clone()).flatten()
                });
                if let Some(error) = step_error {
                    engine.close().await;
                    return Some(error);
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "the step never carried an error: {rows:?}"
                );
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }

        let legacy = run_error(false).await.expect("legacy error");
        let kernel = run_error(true).await.expect("kernel error");
        assert!(
            legacy.contains("\"ecode\":\"boom\""),
            "the legacy step error carries the client's ecode: {legacy}"
        );
        assert_eq!(
            kernel, legacy,
            "the kernel must record the same error as the legacy engine"
        );
    }

    /// The read model across a Back/redo: the kernel projects the *newest*
    /// generation of every node (one row per node), while the legacy rows keep
    /// the redo's history — the backed instance next to the redone one. Both
    /// agree on the surviving instance's state.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn kernel_back_read_model_serves_the_redone_generation() {
        use crate::event::EventAction;

        /// Drive one engine through: start, Back to `s1` when the act is
        /// created, wait for the redone act, then hand back the process's rows.
        async fn run_back(kernel_engine: bool) -> Vec<crate::store::data::Task> {
            const PID: &str = "p_back_rows";
            let workflow = crate::model::Workflow::new()
                .with_id("wf-back-rows")
                .with_step(|step| step.with_id("s1"))
                .with_step(|step| {
                    step.with_id("s2").with_uses(
                        crate::utils::test::USES_IRQ,
                        crate::Vars::new().with("key", "back-act"),
                    )
                });
            let mut config = crate::Config::default();
            config.data.kernel_engine = kernel_engine;
            config.data.tick_interval_secs = Some(1);
            let engine = crate::Engine::builder()
                .set_config(&config)
                .start()
                .await
                .unwrap();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(String, String)>();
            engine.channel().on_message(move |e| {
                let tx = tx.clone();
                async move {
                    if e.r#type == "act"
                        && e.is_params_key("back-act")
                        && e.state == crate::MessageState::Created
                    {
                        let _ = tx.send((e.pid.clone(), e.tid.clone()));
                    }
                }
            });
            let runtime = engine.runtime();
            runtime
                .start(
                    &workflow,
                    crate::Vars::new().with(crate::utils::consts::PROCESS_ID, PID),
                )
                .await
                .unwrap();
            let (pid_now, tid) =
                tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv())
                    .await
                    .expect("the act was never created")
                    .expect("the event channel closed");
            runtime
                .do_action(&crate::event::Action::new(
                    &pid_now,
                    &tid,
                    EventAction::Back,
                    crate::Vars::new().with(crate::utils::consts::ACT_TO, "s1"),
                ))
                .await
                .expect("the act must back");
            // The redo re-runs s1 and parks a *new* s2 instance: its Created
            // message is the point where both engines' rows are settled.
            tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv())
                .await
                .expect("the redone act was never created");
            runtime.cache().flush().await.unwrap();
            let rows = runtime
                .cache()
                .store()
                .tasks()
                .query_all(
                    &crate::store::query::Query::new().filter(
                        crate::store::query::Filter::and()
                            .expr(crate::store::query::Expr::eq("pid", PID.to_string())),
                    ),
                )
                .await
                .unwrap();
            engine.close().await;
            rows
        }

        /// The states the rows name for one *node*: an act row folds to its
        /// owning step, the workflow root is `$` (the legacy `tid`s are random
        /// — only `node_data` names the model node).
        fn states_of(rows: &[crate::store::data::Task], node: &str) -> Vec<String> {
            let node_id = |tid: &str| {
                rows.iter()
                    .find(|row| row.tid == tid)
                    .and_then(|row| {
                        serde_json::from_str::<crate::scheduler::NodeData>(&row.node_data).ok()
                    })
                    .map(|data| data.id)
            };
            let mut states: Vec<String> = rows
                .iter()
                .filter_map(|row| {
                    let row_node = if row.kind == "workflow" {
                        crate::utils::consts::TASK_ROOT_TID.to_string()
                    } else if row.kind == "act" {
                        row.parent
                            .as_deref()
                            .and_then(node_id)
                            .unwrap_or_else(|| row.tid.clone())
                    } else {
                        node_id(&row.tid).unwrap_or_else(|| row.tid.clone())
                    };
                    (row_node == node).then(|| row.state.clone())
                })
                .collect();
            states.sort();
            states.dedup();
            states
        }

        let legacy = run_back(false).await;
        // The legacy rows are one per *instance*, so the redo's history is all
        // there: the replaced step (`completed` — the Back path completes it),
        // its act (`backed`), and the redone pair, whose step runs (`running`)
        // while its act waits (`interrupted`).
        assert_eq!(
            states_of(&legacy, "s2"),
            vec![
                "backed".to_string(),
                "completed".to_string(),
                "interrupted".to_string(),
                "running".to_string()
            ],
            "the legacy rows keep every instance of the node"
        );
        assert_eq!(states_of(&legacy, "s1"), vec!["completed".to_string()]);

        let kernel = run_back(true).await;
        // The kernel's read model is the surviving generation per node: the
        // redone s1 completed, the redone s2 waits again (the act's state, the
        // one the legacy *act* row also shows), and no backed/completed
        // history — the row is the instance the read API addresses.
        assert_eq!(
            states_of(&kernel, "s2"),
            vec!["interrupted".to_string()],
            "the row is the surviving generation, not the history"
        );
        assert_eq!(states_of(&kernel, "s1"), vec!["completed".to_string()]);
        // The root diverges and is registered in the migration notes: the
        // kernel completes the workflow root at its own hop, so a redo leaves
        // it (and the projected process row) `completed` while the run is still
        // in flight, where the legacy root waits for every child instance.
        // Asserting it here would pin that divergence, so this test stops at
        // the nodes both engines agree on.
        assert_eq!(
            kernel
                .iter()
                .filter(|row| row.tid == crate::utils::consts::TASK_ROOT_TID)
                .count(),
            1,
            "one row per node: the root"
        );
        assert_eq!(
            kernel.iter().filter(|row| row.tid == "s1").count(),
            1,
            "one row per node: the redone s1"
        );
        assert_eq!(
            kernel.iter().filter(|row| row.tid == "s2").count(),
            1,
            "one row per node: the redone s2"
        );
    }

    /// A branch predicate that calls an engine function (`$env.WORK_DIR`,
    /// which the legacy branch task read from its process env) must decide the
    /// same branches on both engines — the built-in subset evaluator alone
    /// cannot answer it.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn kernel_env_function_predicate_matches_the_legacy_run() {
        let root =
            std::env::temp_dir().join(format!("acts-kernel-pred-{}", crate::utils::longid()));
        let workflow = || {
            crate::model::Workflow::new()
                .with_id("wf-env-pred")
                .with_step(|step| {
                    step.with_id("step1")
                        .with_branch(|branch| {
                            branch
                                .with_id("b_true")
                                .with_if("$env.WORK_DIR != 'x'")
                                .with_step(|step| step.with_id("s11"))
                        })
                        .with_branch(|branch| {
                            branch
                                .with_id("b_false")
                                .with_if("$env.WORK_DIR == 'x'")
                                .with_step(|step| step.with_id("s21"))
                        })
                })
        };
        let pid = "p_env_pred";
        let mut config = crate::Config::default();
        config.data.tick_interval_secs = Some(1);
        config.data.workdir = Some(root.display().to_string());

        // Legacy: the branch task evaluates `$env.WORK_DIR` in its own env.
        let engine = crate::Engine::builder()
            .set_config(&config)
            .start()
            .await
            .unwrap();
        let settled = engine.signal(());
        let done = settled.clone();
        engine.channel().on_complete(move |_| {
            let done = done.clone();
            async move { done.close() }
        });
        let legacy_proc = engine
            .runtime()
            .start(
                &workflow(),
                crate::Vars::new().with(crate::utils::consts::PROCESS_ID, pid),
            )
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(30), settled.recv())
            .await
            .expect("the legacy run never settled");
        let legacy = folded_legacy_states(&legacy_proc);
        engine.close().await;

        // Kernel driver: the same predicate on a headless task of the engine.
        config.data.kernel_engine = true;
        let engine = crate::Engine::builder()
            .set_config(&config)
            .start()
            .await
            .unwrap();
        let settled = engine.signal(());
        let done = settled.clone();
        engine.channel().on_complete(move |_| {
            let done = done.clone();
            async move { done.close() }
        });
        let runtime = engine.runtime();
        runtime
            .start(
                &workflow(),
                crate::Vars::new().with(crate::utils::consts::PROCESS_ID, pid),
            )
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(30), settled.recv())
            .await
            .expect("the kernel run never settled");
        let host = runtime.kernel().expect("the kernel host").clone();
        let snapshot = host.snapshot(pid).await.unwrap().unwrap();
        assert_parity(&legacy, &snapshot);
        assert_eq!(
            state_of(&snapshot, "b_true"),
            crate::kernel::ExecutionState::Completed
        );
        assert_eq!(
            state_of(&snapshot, "s11"),
            crate::kernel::ExecutionState::Completed
        );
        assert_eq!(
            state_of(&snapshot, "b_false"),
            crate::kernel::ExecutionState::Skipped
        );
        assert!(!has_task(&snapshot, "s21"));

        std::fs::remove_dir_all(&root).ok();
        engine.close().await;
    }

    /// A branch predicate that cannot be evaluated fails its branch (the
    /// legacy branch task's `init` error), the container and its ancestors
    /// walk up `Failed` and the downstream chain skips — the legacy fold must
    /// agree per node.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn kernel_invalid_branch_predicate_matches_the_legacy_run() {
        let workflow = || {
            crate::model::Workflow::new()
                .with_id("wf-invalid-if")
                .with_step(|step| step.with_id("s0"))
                .with_step(|step| {
                    step.with_id("step1")
                        .with_branch(|branch| {
                            branch
                                .with_id("b1")
                                .with_if("missing_name > 0")
                                .with_step(|step| step.with_id("s11"))
                        })
                        .with_branch(|branch| {
                            branch
                                .with_id("b2")
                                .with_else(true)
                                .with_step(|step| step.with_id("s21"))
                        })
                })
                .with_step(|step| step.with_id("s2"))
        };
        let pid = "p_invalid_if";

        // Legacy: the branch task errors, the walk decides the rest.
        let engine = crate::Engine::builder().start().await.unwrap();
        let settled = engine.signal(());
        for handle in [settled.clone(), settled.clone()] {
            engine.channel().on_error(move |_| {
                let handle = handle.clone();
                async move { handle.close() }
            });
        }
        let done = settled.clone();
        engine.channel().on_complete(move |_| {
            let done = done.clone();
            async move { done.close() }
        });
        let legacy_proc = engine
            .runtime()
            .start(
                &workflow(),
                crate::Vars::new().with(crate::utils::consts::PROCESS_ID, pid),
            )
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(30), settled.recv())
            .await
            .expect("the legacy run never settled");
        // The process event fires with the walk; wait until every task is
        // decided (or never started) before folding.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let quiet = engines_quiescent(&engine, pid).await;
            if quiet || std::time::Instant::now() > deadline {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let legacy = folded_legacy_states(&legacy_proc);
        engine.close().await;

        // Kernel driver: the same fold must come out of the aggregate.
        let mut config = crate::Config::default();
        config.data.kernel_engine = true;
        config.data.tick_interval_secs = Some(1);
        let engine = crate::Engine::builder()
            .set_config(&config)
            .start()
            .await
            .unwrap();
        let settled = engine.signal(());
        for handle in [settled.clone(), settled.clone()] {
            engine.channel().on_error(move |_| {
                let handle = handle.clone();
                async move { handle.close() }
            });
        }
        let done = settled.clone();
        engine.channel().on_complete(move |_| {
            let done = done.clone();
            async move { done.close() }
        });
        let runtime = engine.runtime();
        runtime
            .start(
                &workflow(),
                crate::Vars::new().with(crate::utils::consts::PROCESS_ID, pid),
            )
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(30), settled.recv())
            .await
            .expect("the kernel run never settled");
        let host = runtime.kernel().expect("the kernel host").clone();
        let snapshot = host.snapshot(pid).await.unwrap().unwrap();
        assert_parity(&legacy, &snapshot);
        // The failing branch carries the predicate error as its own failure.
        let b1 = crate::kernel::ExecutionId::new("b1", 0).unwrap();
        assert_eq!(
            snapshot
                .executions
                .get(&b1)
                .map(|execution| execution.state),
            Some(crate::kernel::ExecutionState::Failed)
        );
        assert!(
            snapshot
                .variables
                .contains_key(&(b1, "error.message".to_string()))
        );
        engine.close().await;
    }

    /// Drive one engine through a `catches` workflow: complete the `init` act,
    /// then either error the `act1` act with `ecode` or complete it (which is
    /// what dispatches the step's `catches` without an error to answer). The
    /// engine is handed back alive (the caller waits for the nodes it
    /// compares) together with the process the run returned.
    ///
    /// The legacy engine is measurably *wedged* by a caught act error: the
    /// failed act never runs its own `next`, so its propagation phase stays
    /// unapplied and the step's `check_uses_action` gate stops the step's
    /// `next` before it can ever reach `auto_complete`. The handler subtree
    /// still runs (that dispatch is `Step::on_error`'s), which is what the
    /// error tests compare against — the step's own outcome is not, and the
    /// kernel implements the `auto_complete` semantics the legacy code
    /// documents but cannot reach. A successful completion has no such
    /// defect, so that path is compared end to end.
    async fn run_catch_engine(
        kernel_engine: bool,
        pid: &str,
        ecode: Option<&str>,
        workflow: &crate::model::Workflow,
    ) -> (crate::Engine, Arc<crate::scheduler::Process>) {
        let mut config = crate::Config::default();
        config.data.kernel_engine = kernel_engine;
        config.data.tick_interval_secs = Some(1);
        let engine = crate::Engine::builder()
            .set_config(&config)
            .start()
            .await
            .unwrap();
        let runtime = engine.runtime().clone();
        // Subscribe before the start: the act's Created message fires the
        // moment the act runs, and a late subscriber misses it.
        let wait_for_created = |key: &'static str| {
            let sig = engine.signal((String::new(), String::new()));
            let (done, done_rx) = (sig.clone(), sig.clone());
            engine.channel().on_message(move |e| {
                let done = done_rx.clone();
                let key = key.to_string();
                async move {
                    if e.is_params_key(&key) && e.is_state(crate::MessageState::Created) {
                        done.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                        done.close();
                    }
                }
            });
            done
        };
        let init_created = wait_for_created("init");
        let act1_created = wait_for_created("act1");
        let ran = runtime
            .start(
                workflow,
                crate::Vars::new().with(crate::utils::consts::PROCESS_ID, pid),
            )
            .await
            .unwrap();
        let (pid_now, tid) =
            tokio::time::timeout(std::time::Duration::from_secs(30), init_created.recv())
                .await
                .expect("the init act never appeared")
                .clone();
        runtime
            .do_action(&crate::event::Action::new(
                &pid_now,
                &tid,
                crate::event::EventAction::Next,
                crate::Vars::new(),
            ))
            .await
            .expect("the init act must complete");
        let (pid_now, tid) =
            tokio::time::timeout(std::time::Duration::from_secs(30), act1_created.recv())
                .await
                .expect("the act1 act never appeared")
                .clone();
        match ecode {
            Some(ecode) => runtime
                .do_action(&crate::event::Action::new(
                    &pid_now,
                    &tid,
                    crate::event::EventAction::Error,
                    crate::Vars::new()
                        .with(crate::utils::consts::ACT_ERR_CODE, ecode)
                        .with(crate::utils::consts::ACT_ERR_MESSAGE, "boom"),
                ))
                .await
                .expect("the act1 act must error"),
            None => runtime
                .do_action(&crate::event::Action::new(
                    &pid_now,
                    &tid,
                    crate::event::EventAction::Next,
                    crate::Vars::new(),
                ))
                .await
                .expect("the act1 act must complete"),
        };
        (engine, ran)
    }

    /// Wait until every named legacy node has a decided own task (an act's
    /// task lives on its own node id, so this reads the node's own state).
    async fn wait_legacy_nodes(proc: &Arc<crate::scheduler::Process>, nodes: &[&str]) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let decided = nodes.iter().all(|node| {
                let states = legacy_node_states(proc, node);
                !states.is_empty()
                    && states
                        .iter()
                        .all(|state| state != "running" && state != "ready" && state != "pending")
            });
            if decided {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "legacy nodes {nodes:?} never decided: {:?}",
                proc.tasks()
                    .iter()
                    .map(|task| format!("{}:{}", task.node().id(), task.state()))
                    .collect::<Vec<_>>()
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    /// The states of one legacy node's own task instances, sorted.
    fn legacy_node_states(proc: &Arc<crate::scheduler::Process>, node: &str) -> Vec<String> {
        let mut states: Vec<String> = proc
            .tasks()
            .iter()
            .filter(|task| task.node().id() == node)
            .map(|task| task.state().to_string())
            .collect();
        states.sort();
        states
    }

    /// Wait until the kernel aggregate settled (its own completeness
    /// predicate, read from one snapshot so the returned state is the one the
    /// predicate saw), reporting the states when it never does.
    async fn wait_kernel_settled(host: &Arc<KernelHost>, pid: &str) -> Snapshot {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let snapshot = host.snapshot(pid).await.unwrap().expect("aggregate exists");
            if snapshot
                .executions
                .values()
                .all(|execution| execution.state.is_terminal())
            {
                return snapshot;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the kernel run never settled: {:?}",
                snapshot
                    .executions
                    .iter()
                    .map(|(id, execution)| format!("{}:{:?}", id.task_id, execution.state))
                    .collect::<Vec<_>>()
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    /// The `$ecode()`-selected handler answers the error: the handler's branch
    /// runs a msg step. Both engines must route the same way — the legacy one
    /// runs the handler subtree and then wedges at the failed step, which the
    /// kernel completes with the caught error.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn kernel_catch_matches_legacy_when_the_error_is_caught() {
        const PID: &str = "p_catch_caught";
        let workflow = || {
            crate::model::Workflow::new()
                .with_id("wf-catch-caught")
                .with_step(|step| {
                    step.with_id("s0").with_uses(
                        crate::utils::test::USES_IRQ,
                        crate::Vars::new().with("key", "init"),
                    )
                })
                .with_step(|step| {
                    step.with_id("s1")
                        .with_uses(
                            crate::utils::test::USES_IRQ,
                            crate::Vars::new().with("key", "act1"),
                        )
                        .with_catch(|catch| {
                            catch.with_id("c1").with_branch(|branch| {
                                branch
                                    .with_id("b1")
                                    .with_if("$ecode() == 'err1'")
                                    .with_step(|step| {
                                        step.with_id("h1").with_uses(
                                            crate::utils::test::USES_MSG,
                                            crate::Vars::new().with("key", "caught"),
                                        )
                                    })
                            })
                        })
                })
        };
        let (legacy_engine, legacy) = run_catch_engine(false, PID, Some("err1"), &workflow()).await;
        wait_legacy_nodes(&legacy, &["c1", "b1", "h1"]).await;
        // The handler subtree ran the selected branch's msg step; the
        // unselected branch does not exist in this workflow.
        assert_eq!(legacy_node_states(&legacy, "c1"), vec!["completed"]);
        assert_eq!(legacy_node_states(&legacy, "b1"), vec!["completed"]);
        assert_eq!(legacy_node_states(&legacy, "h1"), vec!["completed"]);
        // The catch child carries the legacy `TASK_SIGN = CATCH` the kernel's
        // CATCH mark mirrors.
        assert!(
            legacy
                .task_by_nid("c1")
                .iter()
                .any(|task| task.is_sign(crate::scheduler::Sign::CATCH)),
            "the dispatch pins TASK_SIGN = CATCH on the handler"
        );
        // The measured legacy wedge: the failed act never applied its
        // propagation, so the step's `check_uses_action` gate stops it before
        // `auto_complete` — the step and the run stay running.
        assert_eq!(legacy_node_states(&legacy, "s1"), vec!["running"]);
        assert_eq!(legacy.state().to_string(), "running");

        let (kernel_engine, _ran) = run_catch_engine(true, PID, Some("err1"), &workflow()).await;
        let host = kernel_engine
            .runtime()
            .kernel()
            .expect("the kernel host")
            .clone();
        let snapshot = wait_kernel_settled(&host, PID).await;

        // The parity claim: the handler subtree both engines ran agrees.
        for node in ["c1", "b1", "h1"] {
            assert_eq!(
                legacy_node_states(&legacy, node)
                    .first()
                    .expect("the handler ran")
                    .to_lowercase(),
                format!("{:?}", state_of(&snapshot, node)).to_lowercase(),
                "handler node '{node}' diverged"
            );
        }
        // The kernel's half of the divergence: the caught error completes the
        // step (the legacy `auto_complete` rule the legacy run cannot reach).
        for (node, state) in [
            ("s1", crate::kernel::ExecutionState::Completed),
            (
                crate::utils::consts::TASK_ROOT_TID,
                crate::kernel::ExecutionState::Completed,
            ),
        ] {
            assert_eq!(state_of(&snapshot, node), state, "node '{node}'");
        }
        kernel_engine.close().await;
        legacy_engine.close().await;
    }

    /// A successful completion dispatches the step's `catches` too (the legacy
    /// `check_in_children` schedules every child of a running step, the catch
    /// children included). With no error to answer, `$ecode()` is null: the
    /// `if` handler declines and the `else` handler runs — the legacy branch
    /// selection, and the one catch path both engines reach a terminal state
    /// on, so it is compared end to end.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn kernel_catch_matches_legacy_when_the_step_completes_without_error() {
        const PID: &str = "p_catch_success";
        let workflow = || {
            crate::model::Workflow::new()
                .with_id("wf-catch-success")
                .with_step(|step| {
                    step.with_id("s0").with_uses(
                        crate::utils::test::USES_IRQ,
                        crate::Vars::new().with("key", "init"),
                    )
                })
                .with_step(|step| {
                    step.with_id("s1")
                        .with_uses(
                            crate::utils::test::USES_IRQ,
                            crate::Vars::new().with("key", "act1"),
                        )
                        .with_catch(|catch| {
                            catch
                                .with_id("c1")
                                .with_var("declared", 7)
                                .with_branch(|branch| {
                                    branch
                                        .with_id("b1")
                                        .with_if("$ecode() == 'err1'")
                                        .with_step(|step| {
                                            step.with_id("h1").with_uses(
                                                crate::utils::test::USES_MSG,
                                                crate::Vars::new().with("key", "caught"),
                                            )
                                        })
                                })
                                .with_branch(|branch| {
                                    branch.with_id("b2").with_else(true).with_step(|step| {
                                        step.with_id("h2").with_uses(
                                            crate::utils::test::USES_MSG,
                                            crate::Vars::new().with("key", "others"),
                                        )
                                    })
                                })
                        })
                })
        };
        let (legacy_engine, legacy) = run_catch_engine(false, PID, None, &workflow()).await;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !legacy.state().is_completed() {
            assert!(
                std::time::Instant::now() < deadline,
                "the legacy run never settled: {:?}",
                legacy
                    .tasks()
                    .iter()
                    .map(|task| format!("{}:{}", task.node().id(), task.state()))
                    .collect::<Vec<_>>()
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let legacy_fold = folded_legacy_states(&legacy);

        let (kernel_engine, _ran) = run_catch_engine(true, PID, None, &workflow()).await;
        let host = kernel_engine
            .runtime()
            .kernel()
            .expect("the kernel host")
            .clone();
        let snapshot = wait_kernel_settled(&host, PID).await;
        assert_parity(&legacy_fold, &snapshot);
        // The `if` handler declined and the `else` handler ran, in both
        // engines.
        for (node, state) in [
            ("c1", crate::kernel::ExecutionState::Completed),
            ("b1", crate::kernel::ExecutionState::Skipped),
            ("b2", crate::kernel::ExecutionState::Completed),
            ("h2", crate::kernel::ExecutionState::Completed),
            ("s1", crate::kernel::ExecutionState::Completed),
        ] {
            assert_eq!(state_of(&snapshot, node), state, "node '{node}'");
        }
        assert!(
            !has_task(&snapshot, "h1"),
            "the declining handler never dispatches its act"
        );
        // A nested node's declared vars are seeded by the hop that admits it,
        // not by the aggregate seed (which knows no such execution).
        let c1 = crate::kernel::ExecutionId::new("c1", 0).unwrap();
        assert_eq!(
            snapshot
                .variables
                .get(&(c1, "declared".to_string()))
                .map(String::as_str),
            Some("7"),
            "the fan-out seeds the handler's declared vars"
        );
        kernel_engine.close().await;
        legacy_engine.close().await;
    }

    /// Every handler skipped: the kernel resumes the error after its handlers
    /// settled — the failed step is decided `Failed`, the walk decides the
    /// ancestors and the downstream chain skips. The legacy engine reaches the
    /// skipped dispatch (the same guard) and then wedges at the step.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn kernel_catch_matches_legacy_when_every_handler_skipped() {
        const PID: &str = "p_catch_resumed";
        let workflow = || {
            crate::model::Workflow::new()
                .with_id("wf-catch-resumed")
                .with_step(|step| {
                    step.with_id("s0").with_uses(
                        crate::utils::test::USES_IRQ,
                        crate::Vars::new().with("key", "init"),
                    )
                })
                .with_step(|step| {
                    step.with_id("s1")
                        .with_uses(
                            crate::utils::test::USES_IRQ,
                            crate::Vars::new().with("key", "act1"),
                        )
                        .with_catch(|catch| {
                            catch.with_id("c1").with_if("$ecode() == 'err1'").with_uses(
                                crate::utils::test::USES_MSG,
                                crate::Vars::new().with("key", "caught"),
                            )
                        })
                })
                .with_step(|step| step.with_id("s2"))
        };
        let (legacy_engine, legacy) = run_catch_engine(false, PID, Some("err2"), &workflow()).await;
        wait_legacy_nodes(&legacy, &["c1"]).await;
        // The handler's own guard declined the error. The failed step (and
        // with it the rest of the chain — `s2` is only scheduled once its
        // predecessor settles) stays undecided: the measured legacy wedge.
        assert_eq!(legacy_node_states(&legacy, "c1"), vec!["skipped"]);
        assert_eq!(legacy_node_states(&legacy, "s1"), vec!["running"]);
        assert!(
            legacy_node_states(&legacy, "s2").is_empty(),
            "legacy never schedules the successor of a step it cannot settle"
        );
        assert_eq!(legacy.state().to_string(), "running");

        let (kernel_engine, _ran) = run_catch_engine(true, PID, Some("err2"), &workflow()).await;
        let host = kernel_engine
            .runtime()
            .kernel()
            .expect("the kernel host")
            .clone();
        let snapshot = wait_kernel_settled(&host, PID).await;
        for (node, state) in [
            ("c1", crate::kernel::ExecutionState::Skipped),
            ("s2", crate::kernel::ExecutionState::Skipped),
            ("s1", crate::kernel::ExecutionState::Failed),
            (
                crate::utils::consts::TASK_ROOT_TID,
                crate::kernel::ExecutionState::Failed,
            ),
        ] {
            assert_eq!(state_of(&snapshot, node), state, "node '{node}'");
        }
        assert!(
            !has_task(&snapshot, "h1"),
            "a skipped handler never dispatches its act"
        );
        kernel_engine.close().await;
        legacy_engine.close().await;
    }

    #[tokio::test]
    async fn manual_task_waits_for_the_client_action() {
        let (host, mut wake_rx) = KernelHost::in_memory();
        let mut spec = ProcessSpec::linear("p4", "root", &["wait", "after"]);
        // `wait` has no entry in the auto set: it is a waiting task.
        spec.auto.remove("wait");
        host.start_process(&spec).await.unwrap();
        host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();

        // `wait` ran and is Running; `after` must not have started.
        let snapshot = host.snapshot("p4").await.unwrap().unwrap();
        let state_of = |task: &str| {
            snapshot
                .executions
                .iter()
                .find(|(id, _)| id.task_id == task)
                .map(|(_, e)| e.state)
                .unwrap()
        };
        assert_eq!(state_of("wait"), crate::kernel::ExecutionState::Waiting);
        assert_eq!(state_of("after"), crate::kernel::ExecutionState::Created);

        // The client completes the waiting task; the flow moves on.
        host.apply_action("p4", "wait", ActionKind::Complete, Vec::new())
            .await
            .unwrap();
        host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
        assert!(host.process_completed("p4").await.unwrap());
    }

    /// A step declaring `timeouts` arms a durable timer at its start hop: while
    /// the execution is live, a tick dispatches the first branch that has
    /// neither fired (the one-shot marker) nor is in flight — one per round, in
    /// declaration order, its window evaluated on the engine's environment (the
    /// built-in evaluator decides the plain guards here). The owner may not
    /// complete before the branch it fired settles, and the timer retires once
    /// nothing can fire instead of waking forever.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_timeout_timer_fires_a_branch_once_and_the_owner_waits_for_it() {
        const PID: &str = "p_timeout";
        let (host, mut wake_rx) = KernelHost::in_memory();
        // An unknown package: every `uses` step waits for a client action,
        // without a runner this host has none of.
        let workflow = crate::model::Workflow::new()
            .with_step(|step| {
                step.with_id("s1")
                    .with_uses("acts.test.wait", crate::Vars::new())
                    .with_timeout(|timeout| {
                        timeout
                            .with_id("t1")
                            .with_if("true")
                            .with_uses("acts.test.wait", crate::Vars::new())
                    })
                    .with_timeout(|timeout| timeout.with_id("t2").with_if("false"))
            })
            .with_step(|step| step.with_id("s2"));
        let spec = ProcessSpec::from_workflow(PID, &workflow, &|_| None).unwrap();
        host.start_process(&spec).await.unwrap();
        host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();

        let marker = |snapshot: &Snapshot, node: &str| -> Vec<String> {
            snapshot
                .variables
                .get(&(
                    crate::kernel::ExecutionId::new(node, 0).unwrap(),
                    crate::utils::consts::TASK_TIMEOUTS.to_string(),
                ))
                .map(|raw| serde_json::from_str(raw).unwrap())
                .unwrap_or_default()
        };

        // `s1` waits on the client — the live state a timed-out step is in.
        // The timer's first round rides the start hop's own wake: an open
        // window (`if: true` here) dispatches at once, a closed one
        // (`if: false`) dispatches nothing and stays armed for the next tick.
        let snapshot = host.snapshot(PID).await.unwrap().unwrap();
        assert_eq!(
            state_of(&snapshot, "s1"),
            crate::kernel::ExecutionState::Waiting
        );
        assert_eq!(
            state_of(&snapshot, "t1"),
            crate::kernel::ExecutionState::Waiting,
            "the branch ran into its own client wait"
        );
        assert_eq!(
            marker(&snapshot, "s1"),
            vec!["t1".to_string()],
            "the branch is claimed one-shot"
        );
        assert!(
            !has_task(&snapshot, "t2"),
            "a window that is not open dispatches nothing"
        );
        // The legacy task data the tick freezes: the owner's cost start and
        // the frozen cost, and the branch reads the cost through its scope
        // chain exactly like a legacy timeout child read its parent's.
        let owner = crate::kernel::ExecutionId::new("s1", 0).unwrap();
        assert!(
            snapshot
                .variables
                .contains_key(&(owner.clone(), crate::kernel::STARTED_VAR.to_string())),
            "the start hop stamps the cost start"
        );
        assert!(
            snapshot
                .variables
                .contains_key(&(owner, crate::utils::consts::TASK_COST.to_string())),
            "the tick freezes the cost (the legacy __cost datum)"
        );
        assert!(
            scope_vars(
                &snapshot,
                &crate::kernel::ExecutionId::new("t1", 0).unwrap()
            )
            .iter()
            .any(|(name, _)| name == crate::utils::consts::TASK_COST),
            "a timeout branch reads the frozen cost"
        );

        // A later tick re-evaluates (the timer stays armed) without ever
        // dispatching a branch that already fired.
        host.recover(SystemClock.now_ms()).await.unwrap();
        host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
        let snapshot = host.snapshot(PID).await.unwrap().unwrap();
        assert_eq!(
            executions_of(&snapshot, "t1").len(),
            1,
            "a one-shot branch fires once"
        );
        assert_eq!(marker(&snapshot, "s1"), vec!["t1".to_string()]);

        // The owner's act completing while its branch runs is only the act:
        // the step waits for the branch and the chain successor keeps waiting.
        host.apply_action(PID, "s1", ActionKind::Complete, Vec::new())
            .await
            .unwrap();
        host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
        let snapshot = host.snapshot(PID).await.unwrap().unwrap();
        assert_eq!(
            state_of(&snapshot, "s1"),
            crate::kernel::ExecutionState::Waiting,
            "the step waits for the branch it fired"
        );
        assert_eq!(
            state_of(&snapshot, "s2"),
            crate::kernel::ExecutionState::Created
        );

        // The branch settles: the join completes the owner, the flow moves on,
        // and the next tick retires the timer.
        host.apply_action(PID, "t1", ActionKind::Complete, Vec::new())
            .await
            .unwrap();
        host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
        host.recover(SystemClock.now_ms()).await.unwrap();
        host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
        let snapshot = host.snapshot(PID).await.unwrap().unwrap();
        assert_eq!(
            state_of(&snapshot, "s1"),
            crate::kernel::ExecutionState::Completed
        );
        assert_eq!(
            state_of(&snapshot, "s2"),
            crate::kernel::ExecutionState::Completed
        );
        assert!(host.process_completed(PID).await.unwrap());
        assert!(
            snapshot
                .operations
                .values()
                .filter(|operation| operation.kind == OperationKind::DispatchTimeout)
                .all(|operation| matches!(operation.state, OperationState::Completed(_))),
            "a decided execution retires its timer"
        );
    }

    /// A step may declare both `branches` and `timeouts`: the tick fires a
    /// branch while the step is still waiting on its act, and the act's
    /// completion must still fan the step's declared branches out — a fired
    /// timeout branch is not an "already fanned out" container child. The step
    /// then waits for both child sets (the legacy `auto_complete` counts every
    /// child task) and joins when the last one settles.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_timeout_branch_does_not_mask_the_branches_of_the_same_step() {
        const PID: &str = "p_timeout_branch";
        let (host, mut wake_rx) = KernelHost::in_memory();
        let workflow = crate::model::Workflow::new().with_step(|step| {
            step.with_id("s1")
                .with_uses("acts.test.wait", crate::Vars::new())
                .with_branch(|branch| {
                    branch
                        .with_id("b1")
                        .with_if("true")
                        .with_step(|step| step.with_id("s11"))
                })
                .with_timeout(|timeout| {
                    timeout
                        .with_id("t1")
                        .with_if("true")
                        .with_uses("acts.test.wait", crate::Vars::new())
                })
        });
        let spec = ProcessSpec::from_workflow(PID, &workflow, &|_| None).unwrap();
        host.start_process(&spec).await.unwrap();
        host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();

        let snapshot = host.snapshot(PID).await.unwrap().unwrap();
        assert_eq!(
            state_of(&snapshot, "s1"),
            crate::kernel::ExecutionState::Waiting
        );
        assert_eq!(
            state_of(&snapshot, "t1"),
            crate::kernel::ExecutionState::Waiting,
            "the timer fired its branch while the step waited on its act"
        );
        assert!(
            !has_task(&snapshot, "b1"),
            "the declared branches fan out on the act's completion"
        );

        // The act's completion: the step is not done (t1 is in flight) but its
        // branch child set comes alive.
        host.apply_action(PID, "s1", ActionKind::Complete, Vec::new())
            .await
            .unwrap();
        host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
        let snapshot = host.snapshot(PID).await.unwrap().unwrap();
        assert_eq!(
            state_of(&snapshot, "s1"),
            crate::kernel::ExecutionState::Waiting
        );
        assert_eq!(
            state_of(&snapshot, "b1"),
            crate::kernel::ExecutionState::Completed,
            "the selected branch ran (its own inner step is auto)"
        );

        // The last child settles: the join completes the step.
        host.apply_action(PID, "t1", ActionKind::Complete, Vec::new())
            .await
            .unwrap();
        host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
        let snapshot = host.snapshot(PID).await.unwrap().unwrap();
        assert_eq!(
            state_of(&snapshot, "s1"),
            crate::kernel::ExecutionState::Completed
        );
        assert!(host.process_completed(PID).await.unwrap());
    }

    #[tokio::test]
    async fn completing_a_terminal_task_is_rejected_without_consuming_state() {
        let (host, mut wake_rx) = KernelHost::in_memory();
        let spec = ProcessSpec::linear("p5", "root", &[]);
        host.start_process(&spec).await.unwrap();
        host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();
        assert!(host.process_completed("p5").await.unwrap());
        let err = host
            .apply_action("p5", "$", ActionKind::Complete, Vec::new())
            .await
            .unwrap_err();
        let before = host.snapshot("p5").await.unwrap().unwrap();
        // The rejection text is the planner's; the contract is the refusal
        // plus a frozen aggregate.
        assert!(!format!("{err}").is_empty());
        let after = host.snapshot("p5").await.unwrap().unwrap();
        assert_eq!(
            before.version, after.version,
            "a rejected action must not move the aggregate"
        );
    }

    /// The skip cascade: cancelling the waiting head of a chain skips the
    /// rest instead of erroring the propagation. Every execution ends
    /// terminal, and the skipped task is identifiable by its state.
    #[tokio::test]
    async fn cancelled_head_cascades_skip_through_the_chain() {
        let (host, mut wake_rx) = KernelHost::in_memory();
        let mut spec = ProcessSpec::linear("p6", "root", &["head", "mid", "tail"]);
        spec.auto.remove("head"); // the head waits for a client action
        host.start_process(&spec).await.unwrap();
        host.run_until_quiescent(&mut wake_rx, 100).await.unwrap();

        host.apply_action("p6", "head", ActionKind::Cancel, Vec::new())
            .await
            .unwrap();
        host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();

        // The rewind: head gen0 is Cancelled, the downstream is Skipped, and
        // a fresh head generation is already parked waiting again.
        let snapshot = host.snapshot("p6").await.unwrap().unwrap();
        let state_of = |task: &str, generation: u64| {
            snapshot
                .executions
                .get(&crate::kernel::ExecutionId::new(task, generation).unwrap())
                .map(|execution| execution.state)
                .unwrap()
        };
        assert_eq!(
            state_of("head", 0),
            crate::kernel::ExecutionState::Cancelled
        );
        assert_eq!(state_of("mid", 0), crate::kernel::ExecutionState::Skipped);
        assert_eq!(state_of("tail", 0), crate::kernel::ExecutionState::Skipped);
        assert_eq!(state_of("head", 1), crate::kernel::ExecutionState::Waiting);

        // The re-run answers its own action; the recreated downstream runs.
        host.apply_action("p6", "head", ActionKind::Complete, Vec::new())
            .await
            .unwrap();
        host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
        assert!(host.process_completed("p6").await.unwrap());
        let snapshot = host.snapshot("p6").await.unwrap().unwrap();
        assert_eq!(
            snapshot
                .executions
                .get(&crate::kernel::ExecutionId::new("head", 1).unwrap())
                .map(|execution| execution.state),
            Some(crate::kernel::ExecutionState::Completed)
        );
    }

    // ---- nested model import: the legacy branch-if suite, mirrored ----

    type Snapshot = crate::kernel::AggregateSnapshot;

    fn state_of(snapshot: &Snapshot, task: &str) -> crate::kernel::ExecutionState {
        snapshot
            .executions
            .iter()
            .filter(|(_, execution)| execution.model_node.as_str() == task)
            .max_by_key(|(id, _)| id.generation)
            .map(|(_, execution)| execution.state)
            .unwrap_or_else(|| panic!("execution for node '{task}' missing"))
    }

    fn has_task(snapshot: &Snapshot, task: &str) -> bool {
        snapshot
            .executions
            .values()
            .any(|execution| execution.model_node.as_str() == task)
    }

    async fn run_branched(workflow: &crate::model::Workflow, pid: &str) -> Snapshot {
        let (host, mut wake_rx) = KernelHost::in_memory();
        let spec = ProcessSpec::from_workflow(pid, workflow, &|_| None).unwrap();
        host.start_process(&spec).await.unwrap();
        let report = host.run_until_quiescent(&mut wake_rx, 500).await.unwrap();
        assert_eq!(report.retried, 0, "every hop must land: {report:?}");
        host.snapshot(pid).await.unwrap().unwrap()
    }

    /// A branch whose `if` holds runs its subtree; the `else` sibling is one
    /// Skipped execution and never creates its own steps (the legacy parity
    /// floor: one terminal task per model node that legacy created).
    #[tokio::test]
    async fn branched_step_selects_the_true_branch_and_skips_the_else() {
        let workflow = crate::model::Workflow::new()
            .with_var("v", 1)
            .with_step(|step| {
                step.with_id("step1")
                    .with_branch(|branch| {
                        branch
                            .with_id("b1")
                            .with_if("v > 0")
                            .with_step(|step| step.with_id("s11"))
                    })
                    .with_branch(|branch| {
                        branch
                            .with_id("b2")
                            .with_else(true)
                            .with_step(|step| step.with_id("s21"))
                    })
            });
        let snapshot = run_branched(&workflow, "p-if-true").await;
        assert!(host_all_terminal(&snapshot));
        assert_eq!(
            state_of(&snapshot, "step1"),
            crate::kernel::ExecutionState::Completed
        );
        assert_eq!(
            state_of(&snapshot, "b1"),
            crate::kernel::ExecutionState::Completed
        );
        assert_eq!(
            state_of(&snapshot, "s11"),
            crate::kernel::ExecutionState::Completed
        );
        assert_eq!(
            state_of(&snapshot, "b2"),
            crate::kernel::ExecutionState::Skipped
        );
        assert!(
            !has_task(&snapshot, "s21"),
            "unselected branch steps are not created"
        );
    }

    /// No `if` matches: the `else` branch runs.
    #[tokio::test]
    async fn else_branch_runs_when_no_condition_matches() {
        let workflow = crate::model::Workflow::new()
            .with_var("v", 1)
            .with_step(|step| {
                step.with_id("step1")
                    .with_branch(|branch| {
                        branch
                            .with_id("b1")
                            .with_if("v < 0")
                            .with_step(|step| step.with_id("s11"))
                    })
                    .with_branch(|branch| {
                        branch
                            .with_id("b2")
                            .with_else(true)
                            .with_step(|step| step.with_id("s21"))
                    })
            });
        let snapshot = run_branched(&workflow, "p-else").await;
        assert!(host_all_terminal(&snapshot));
        assert_eq!(
            state_of(&snapshot, "b1"),
            crate::kernel::ExecutionState::Skipped
        );
        assert!(!has_task(&snapshot, "s11"));
        assert_eq!(
            state_of(&snapshot, "b2"),
            crate::kernel::ExecutionState::Completed
        );
        assert_eq!(
            state_of(&snapshot, "s21"),
            crate::kernel::ExecutionState::Completed
        );
    }

    /// A branch with neither `if` nor `else` skips; the flow still finishes.
    #[tokio::test]
    async fn branch_without_if_or_else_skips_and_the_flow_finishes() {
        let workflow = crate::model::Workflow::new().with_step(|step| {
            step.with_id("step1").with_branch(|branch| {
                branch
                    .with_id("b1")
                    .with_step(|step| step.with_id("s11"))
                    .with_step(|step| step.with_id("s12"))
            })
        });
        let snapshot = run_branched(&workflow, "p-empty-if").await;
        assert!(host_all_terminal(&snapshot));
        assert_eq!(
            state_of(&snapshot, "b1"),
            crate::kernel::ExecutionState::Skipped
        );
        assert!(!has_task(&snapshot, "s11") && !has_task(&snapshot, "s12"));
        assert_eq!(
            state_of(&snapshot, "step1"),
            crate::kernel::ExecutionState::Completed
        );
    }

    /// Every `true` branch runs — legacy evaluates each branch independently.
    #[tokio::test]
    async fn multiple_true_branches_all_run() {
        let workflow = crate::model::Workflow::new()
            .with_var("v", 1)
            .with_step(|step| {
                step.with_id("step1")
                    .with_branch(|branch| {
                        branch
                            .with_id("b1")
                            .with_if("v > 0")
                            .with_step(|step| step.with_id("s11"))
                    })
                    .with_branch(|branch| {
                        branch
                            .with_id("b2")
                            .with_if("v >= 1")
                            .with_step(|step| step.with_id("s21"))
                    })
            });
        let snapshot = run_branched(&workflow, "p-multi").await;
        assert!(host_all_terminal(&snapshot));
        assert_eq!(
            state_of(&snapshot, "b1"),
            crate::kernel::ExecutionState::Completed
        );
        assert_eq!(
            state_of(&snapshot, "b2"),
            crate::kernel::ExecutionState::Completed
        );
        assert!(has_task(&snapshot, "s11") && has_task(&snapshot, "s21"));
    }

    /// The flow continues past the branched step to the next chain step, and
    /// a branch's inner chain runs in order before the join completes.
    #[tokio::test]
    async fn branched_step_flows_to_the_next_chain_step() {
        let workflow = crate::model::Workflow::new()
            .with_var("v", 1)
            .with_step(|step| {
                step.with_id("step1").with_branch(|branch| {
                    branch
                        .with_id("b1")
                        .with_if("v > 0")
                        .with_step(|step| step.with_id("s11"))
                        .with_step(|step| step.with_id("s12"))
                })
            })
            .with_step(|step| step.with_id("step2"));
        let snapshot = run_branched(&workflow, "p-chain").await;
        assert!(host_all_terminal(&snapshot));
        assert_eq!(
            state_of(&snapshot, "s12"),
            crate::kernel::ExecutionState::Completed
        );
        assert_eq!(
            state_of(&snapshot, "step1"),
            crate::kernel::ExecutionState::Completed
        );
        assert_eq!(
            state_of(&snapshot, "step2"),
            crate::kernel::ExecutionState::Completed
        );
    }

    /// Nesting composes: a branch containing a step that branches again.
    #[tokio::test]
    async fn nested_branched_step_inside_a_branch() {
        let workflow = crate::model::Workflow::new()
            .with_var("v", 1)
            .with_step(|step| {
                step.with_id("step1").with_branch(|branch| {
                    branch.with_id("b1").with_if("v > 0").with_step(|step| {
                        step.with_id("mid")
                            .with_branch(|branch| {
                                branch
                                    .with_id("c1")
                                    .with_if("v > 0")
                                    .with_step(|step| step.with_id("c1s"))
                            })
                            .with_branch(|branch| {
                                branch
                                    .with_id("c2")
                                    .with_else(true)
                                    .with_step(|step| step.with_id("c2s"))
                            })
                    })
                })
            });
        let snapshot = run_branched(&workflow, "p-nested").await;
        assert!(host_all_terminal(&snapshot));
        assert_eq!(
            state_of(&snapshot, "b1"),
            crate::kernel::ExecutionState::Completed
        );
        assert_eq!(
            state_of(&snapshot, "mid"),
            crate::kernel::ExecutionState::Completed
        );
        assert_eq!(
            state_of(&snapshot, "c1"),
            crate::kernel::ExecutionState::Completed
        );
        assert_eq!(
            state_of(&snapshot, "c2"),
            crate::kernel::ExecutionState::Skipped
        );
        assert!(!has_task(&snapshot, "c2s"));
    }

    /// A branch's waiting (IRQ) step parks the whole container chain; the
    /// client completion resumes it through the joins.
    #[tokio::test]
    async fn waiting_inner_step_resumes_on_the_client_action() {
        let workflow = crate::model::Workflow::new()
            .with_var("v", 1)
            .with_step(|step| {
                step.with_id("step1").with_branch(|branch| {
                    branch.with_id("b1").with_if("v > 0").with_step(|step| {
                        step.with_id("s11")
                            .with_uses(crate::utils::test::USES_IRQ, crate::Vars::new())
                    })
                })
            });
        let (host, mut wake_rx) = KernelHost::in_memory();
        let spec = ProcessSpec::from_workflow("p-wait", &workflow, &|_| None).unwrap();
        host.start_process(&spec).await.unwrap();
        host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();

        let snapshot = host.snapshot("p-wait").await.unwrap().unwrap();
        assert_eq!(
            state_of(&snapshot, "s11"),
            crate::kernel::ExecutionState::Waiting
        );
        assert_eq!(
            state_of(&snapshot, "b1"),
            crate::kernel::ExecutionState::Waiting
        );
        assert_eq!(
            state_of(&snapshot, "step1"),
            crate::kernel::ExecutionState::Waiting
        );

        host.apply_action("p-wait", "s11", ActionKind::Complete, Vec::new())
            .await
            .unwrap();
        host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
        assert!(host.process_completed("p-wait").await.unwrap());
        let snapshot = host.snapshot("p-wait").await.unwrap().unwrap();
        assert_eq!(
            state_of(&snapshot, "b1"),
            crate::kernel::ExecutionState::Completed
        );
        assert_eq!(
            state_of(&snapshot, "step1"),
            crate::kernel::ExecutionState::Completed
        );
    }

    /// A client completion of a waiting branched step fans out with the
    /// action's inputs on the container's scope.
    #[tokio::test]
    async fn waiting_branched_step_fans_out_on_the_client_completion() {
        let workflow = crate::model::Workflow::new().with_step(|step| {
            step.with_id("step1")
                .with_uses(crate::utils::test::USES_IRQ, crate::Vars::new())
                .with_branch(|branch| {
                    branch
                        .with_id("b1")
                        .with_if("w > 0")
                        .with_step(|step| step.with_id("s11"))
                })
        });
        let (host, mut wake_rx) = KernelHost::in_memory();
        let spec = ProcessSpec::from_workflow("p-manual", &workflow, &|_| None).unwrap();
        host.start_process(&spec).await.unwrap();
        host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
        assert!(
            !has_task(&host.snapshot("p-manual").await.unwrap().unwrap(), "b1"),
            "no fan-out before the client decides"
        );

        host.apply_action(
            "p-manual",
            "step1",
            ActionKind::Complete,
            vec![("w".to_string(), "2".to_string())],
        )
        .await
        .unwrap();
        host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
        assert!(host.process_completed("p-manual").await.unwrap());
        let snapshot = host.snapshot("p-manual").await.unwrap().unwrap();
        assert_eq!(
            state_of(&snapshot, "b1"),
            crate::kernel::ExecutionState::Completed
        );
        assert_eq!(
            state_of(&snapshot, "s11"),
            crate::kernel::ExecutionState::Completed
        );
    }

    /// Cancelling before the fan-out rewinds the chain; the re-entered step
    /// fans out fresh generations of its branches when it completes again.
    #[tokio::test]
    async fn cancel_before_fanout_reruns_branches_as_fresh_generations() {
        let workflow = crate::model::Workflow::new()
            .with_var("v", 1)
            .with_step(|step| {
                step.with_id("step1")
                    .with_uses(crate::utils::test::USES_IRQ, crate::Vars::new())
                    .with_branch(|branch| {
                        branch
                            .with_id("b1")
                            .with_if("v > 0")
                            .with_step(|step| step.with_id("s11"))
                    })
            })
            .with_step(|step| step.with_id("step2"));
        let (host, mut wake_rx) = KernelHost::in_memory();
        let spec = ProcessSpec::from_workflow("p-cancel-early", &workflow, &|_| None).unwrap();
        host.start_process(&spec).await.unwrap();
        host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();

        host.apply_action("p-cancel-early", "step1", ActionKind::Cancel, Vec::new())
            .await
            .unwrap();
        host.run_until_quiescent(&mut wake_rx, 300).await.unwrap();

        // The re-entered step waits again; completing it fans out branches
        // for the first time (generation 0 — none existed to rewind).
        host.apply_action("p-cancel-early", "step1", ActionKind::Complete, Vec::new())
            .await
            .unwrap();
        host.run_until_quiescent(&mut wake_rx, 300).await.unwrap();
        assert!(host.process_completed("p-cancel-early").await.unwrap());

        let snapshot = host.snapshot("p-cancel-early").await.unwrap().unwrap();
        assert!(host_all_terminal(&snapshot));
        assert_eq!(
            state_of(&snapshot, "b1"),
            crate::kernel::ExecutionState::Completed
        );
        assert_eq!(
            state_of(&snapshot, "s11"),
            crate::kernel::ExecutionState::Completed
        );
        assert_eq!(
            state_of(&snapshot, "step2"),
            crate::kernel::ExecutionState::Completed
        );
        assert_eq!(
            executions_of(&snapshot, "step1").len(),
            2,
            "gen0 cancelled + gen1 completed"
        );
    }

    /// Cancelling mid-branches: the cascade closes the in-flight branch
    /// subtree, the rewind recreates it as fresh generations, and the
    /// fan-out READMITS those instead of allocating duplicates.
    #[tokio::test]
    async fn cancel_mid_branches_cascades_then_reenters_through_readmission() {
        let workflow = crate::model::Workflow::new()
            .with_var("v", 1)
            .with_step(|step| {
                step.with_id("step1")
                    .with_branch(|branch| {
                        branch.with_id("b1").with_if("v > 0").with_step(|step| {
                            step.with_id("s11")
                                .with_uses(crate::utils::test::USES_IRQ, crate::Vars::new())
                        })
                    })
                    .with_branch(|branch| {
                        branch
                            .with_id("b2")
                            .with_else(true)
                            .with_step(|step| step.with_id("s21"))
                    })
            })
            .with_step(|step| step.with_id("step2"));
        let (host, mut wake_rx) = KernelHost::in_memory();
        let spec = ProcessSpec::from_workflow("p-cancel-mid", &workflow, &|_| None).unwrap();
        host.start_process(&spec).await.unwrap();
        host.run_until_quiescent(&mut wake_rx, 300).await.unwrap();

        // b1/s11 wait mid-branches; the else branch skipped; step1 is parked
        // by the fan-out.
        let snapshot = host.snapshot("p-cancel-mid").await.unwrap().unwrap();
        assert_eq!(
            state_of(&snapshot, "s11"),
            crate::kernel::ExecutionState::Waiting
        );
        assert_eq!(
            state_of(&snapshot, "step1"),
            crate::kernel::ExecutionState::Waiting
        );
        assert_eq!(
            state_of(&snapshot, "b2"),
            crate::kernel::ExecutionState::Skipped
        );

        host.apply_action("p-cancel-mid", "step1", ActionKind::Cancel, Vec::new())
            .await
            .unwrap();
        host.run_until_quiescent(&mut wake_rx, 400).await.unwrap();

        // The gen0 branch subtree is closed by the cascade, and the rewind
        // has already recreated the branches as Created generations.
        let snapshot = host.snapshot("p-cancel-mid").await.unwrap().unwrap();
        assert!(host_all_terminal_generations_0(&snapshot));
        // step1 is auto: the re-entered generation re-fans out on its own,
        // READMITTING the rewound branch generations (never a third one).
        assert_eq!(
            executions_of(&snapshot, "b1"),
            vec![0, 1],
            "readmission must not allocate a third generation"
        );
        assert_eq!(
            state_of(&snapshot, "b1"),
            crate::kernel::ExecutionState::Waiting
        );
        assert_eq!(
            state_of(&snapshot, "s11"),
            crate::kernel::ExecutionState::Waiting
        );

        host.apply_action("p-cancel-mid", "s11", ActionKind::Complete, Vec::new())
            .await
            .unwrap();
        host.run_until_quiescent(&mut wake_rx, 400).await.unwrap();
        assert!(host.process_completed("p-cancel-mid").await.unwrap());
        let snapshot = host.snapshot("p-cancel-mid").await.unwrap().unwrap();
        assert!(host_all_terminal(&snapshot));
        assert_eq!(
            state_of(&snapshot, "b1"),
            crate::kernel::ExecutionState::Completed
        );
        assert_eq!(
            state_of(&snapshot, "step2"),
            crate::kernel::ExecutionState::Completed
        );
    }

    /// The parity floor for branches: the same branched workflow settles on
    /// both engines with the same per-node terminal states.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn kernel_path_matches_legacy_for_branch_selection() {
        let workflow = crate::model::Workflow::new()
            .with_id("wf-branch-parity")
            .with_var("v", 1)
            .with_step(|step| {
                step.with_id("step1")
                    .with_branch(|branch| {
                        branch
                            .with_id("b1")
                            .with_if("v > 0")
                            .with_step(|step| step.with_id("s11"))
                    })
                    .with_branch(|branch| {
                        branch
                            .with_id("b2")
                            .with_else(true)
                            .with_step(|step| step.with_id("s21"))
                    })
            })
            .with_step(|step| step.with_id("step2"));

        // Legacy engine.
        let engine = crate::Engine::builder().start().await.unwrap();
        let sig = engine.signal(());
        let done = sig.clone();
        engine.channel().on_complete(move |_| {
            let done = done.clone();
            async move { done.close() }
        });
        let legacy_proc = engine
            .runtime()
            .start(&workflow.clone(), crate::Vars::new())
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(30), sig.recv())
            .await
            .expect("legacy engine never finished");
        let mut legacy: Vec<(String, String)> = legacy_proc
            .tasks()
            .iter()
            .map(|task| {
                let state = match task.state() {
                    crate::TaskState::Completed | crate::TaskState::Submitted => "Completed",
                    crate::TaskState::Skipped => "Skipped",
                    other => {
                        panic!("unexpected legacy terminal state {other:?}")
                    }
                };
                // The kernel keys executions by model node id; the legacy
                // root task's node carries the workflow id where the kernel
                // uses the `$` root.
                let node = if task.node().kind() == crate::NodeKind::Workflow {
                    crate::utils::consts::TASK_ROOT_TID.to_string()
                } else {
                    task.node().id().to_string()
                };
                (node, state.to_string())
            })
            .collect();
        legacy.sort();

        // Kernel host.
        let snapshot = run_branched(&workflow, "k-branch-parity").await;
        let mut kernel: Vec<(String, String)> = snapshot
            .executions
            .values()
            .map(|execution| {
                (
                    execution.model_node.as_str().to_string(),
                    format!("{:?}", execution.state),
                )
            })
            .collect();
        kernel.sort();

        assert_eq!(legacy, kernel, "terminal node set and states must match");
    }

    fn host_all_terminal(snapshot: &Snapshot) -> bool {
        snapshot.executions.values().all(|e| e.state.is_terminal())
            && snapshot
                .operations
                .values()
                .all(|record| matches!(record.state, OperationState::Completed(_)))
    }

    /// Generation-0-only terminality: a rewound run keeps fresh generations
    /// in flight, so only the closed generations must be terminal.
    fn host_all_terminal_generations_0(snapshot: &Snapshot) -> bool {
        snapshot
            .executions
            .iter()
            .filter(|(id, _)| id.generation == 0)
            .all(|(_, execution)| execution.state.is_terminal())
    }

    fn executions_of(snapshot: &Snapshot, task: &str) -> Vec<u64> {
        let mut generations: Vec<u64> = snapshot
            .executions
            .keys()
            .filter(|id| id.task_id == task)
            .map(|id| id.generation)
            .collect();
        generations.sort_unstable();
        generations
    }

    // ---- parity matrix: Abort / Back / Fail (Error) ----

    /// Abort × IRQ: aborting a waiting step decides it `Aborted`, rewrites
    /// its ancestor chain `Aborted` (the legacy `abort_task` walk — the root
    /// included, terminal ancestors not spared), skips the untouched
    /// downstream, and leaves the completed sibling alone.
    #[tokio::test]
    async fn abort_on_waiting_step_aborts_the_ancestor_chain() {
        let workflow = crate::model::Workflow::new()
            .with_step(|step| step.with_id("s1"))
            .with_step(|step| {
                step.with_id("s2").with_uses(
                    crate::utils::test::USES_IRQ,
                    crate::Vars::new().with("key", "abort-act"),
                )
            })
            .with_step(|step| step.with_id("s3"));
        let (host, mut wake_rx) = KernelHost::in_memory();
        let spec = ProcessSpec::from_workflow("p-abort", &workflow, &|_| None).unwrap();
        host.start_process(&spec).await.unwrap();
        host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();

        host.apply_action("p-abort", "s2", ActionKind::Abort, Vec::new())
            .await
            .unwrap();
        host.run_until_quiescent(&mut wake_rx, 300).await.unwrap();

        let snapshot = host.snapshot("p-abort").await.unwrap().unwrap();
        assert!(host_all_terminal(&snapshot));
        let state = |task: &str| state_of(&snapshot, task);
        assert_eq!(state("s2"), crate::kernel::ExecutionState::Aborted);
        // The ancestor walk: the root — terminal since the seed — is
        // rewritten Aborted; the completed sibling is not an ancestor.
        assert_eq!(state("$"), crate::kernel::ExecutionState::Aborted);
        assert_eq!(state("s1"), crate::kernel::ExecutionState::Completed);
        // The untouched downstream joins the death cascade.
        assert_eq!(state("s3"), crate::kernel::ExecutionState::Skipped);
    }

    /// Abort × branches: aborting a waiting branch step ends the whole
    /// container chain upward (branch, branched step, root) while the
    /// already-settled sibling branch keeps its state.
    #[tokio::test]
    async fn abort_inside_a_branch_aborts_the_container_chain() {
        let workflow = crate::model::Workflow::new()
            .with_var("v", 1)
            .with_step(|step| {
                step.with_id("step1")
                    .with_branch(|branch| {
                        branch.with_id("b1").with_if("v > 0").with_step(|step| {
                            step.with_id("s11")
                                .with_uses(crate::utils::test::USES_IRQ, crate::Vars::new())
                        })
                    })
                    .with_branch(|branch| {
                        branch
                            .with_id("b2")
                            .with_else(true)
                            .with_step(|step| step.with_id("s21"))
                    })
            });
        let (host, mut wake_rx) = KernelHost::in_memory();
        let spec = ProcessSpec::from_workflow("p-abort-branch", &workflow, &|_| None).unwrap();
        host.start_process(&spec).await.unwrap();
        host.run_until_quiescent(&mut wake_rx, 300).await.unwrap();

        host.apply_action("p-abort-branch", "s11", ActionKind::Abort, Vec::new())
            .await
            .unwrap();
        host.run_until_quiescent(&mut wake_rx, 400).await.unwrap();

        let snapshot = host.snapshot("p-abort-branch").await.unwrap().unwrap();
        assert!(host_all_terminal(&snapshot));
        let state = |task: &str| state_of(&snapshot, task);
        assert_eq!(state("s11"), crate::kernel::ExecutionState::Aborted);
        assert_eq!(state("b1"), crate::kernel::ExecutionState::Aborted);
        assert_eq!(state("step1"), crate::kernel::ExecutionState::Aborted);
        assert_eq!(state("$"), crate::kernel::ExecutionState::Aborted);
        assert_eq!(state("b2"), crate::kernel::ExecutionState::Skipped);
    }

    /// Back × IRQ: backing the waiting step to its completed predecessor
    /// leaves the old generation `Backed`, re-runs the named step as a fresh
    /// generation, and recreates the downstream behind it.
    #[tokio::test]
    async fn back_redo_reruns_the_named_step_and_its_downstream() {
        let workflow = crate::model::Workflow::new()
            .with_step(|step| step.with_id("s1"))
            .with_step(|step| {
                step.with_id("s2").with_uses(
                    crate::utils::test::USES_IRQ,
                    crate::Vars::new().with("key", "back-act"),
                )
            });
        let (host, mut wake_rx) = KernelHost::in_memory();
        let spec = ProcessSpec::from_workflow("p-back", &workflow, &|_| None).unwrap();
        host.start_process(&spec).await.unwrap();
        host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();

        host.apply_action(
            "p-back",
            "s2",
            ActionKind::Back,
            vec![("to".to_string(), "s1".to_string())],
        )
        .await
        .unwrap();
        host.run_until_quiescent(&mut wake_rx, 300).await.unwrap();

        let snapshot = host.snapshot("p-back").await.unwrap().unwrap();
        let state_at = |task: &str, generation: u64| {
            snapshot
                .executions
                .get(&crate::kernel::ExecutionId::new(task, generation).unwrap())
                .map(|execution| execution.state)
                .unwrap()
        };
        assert_eq!(
            state_at("s2", 0),
            crate::kernel::ExecutionState::Backed,
            "the backed generation keeps its decision"
        );
        assert_eq!(state_at("s1", 0), crate::kernel::ExecutionState::Completed);
        // The redo: s1 generation 1 already ran; s2 generation 1 waits again.
        assert_eq!(state_at("s1", 1), crate::kernel::ExecutionState::Completed);
        assert_eq!(state_at("s2", 1), crate::kernel::ExecutionState::Waiting);

        host.apply_action("p-back", "s2", ActionKind::Complete, Vec::new())
            .await
            .unwrap();
        host.run_until_quiescent(&mut wake_rx, 300).await.unwrap();
        assert!(host.process_completed("p-back").await.unwrap());
    }

    /// Back without the client's `to` is refused before anything durable.
    #[tokio::test]
    async fn back_without_to_is_refused_without_consuming_state() {
        let workflow = crate::model::Workflow::new().with_step(|step| {
            step.with_id("s1").with_uses(
                crate::utils::test::USES_IRQ,
                crate::Vars::new().with("key", "back-no-to"),
            )
        });
        let (host, mut wake_rx) = KernelHost::in_memory();
        let spec = ProcessSpec::from_workflow("p-back-no-to", &workflow, &|_| None).unwrap();
        host.start_process(&spec).await.unwrap();
        host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
        let before = host.snapshot("p-back-no-to").await.unwrap().unwrap();

        let error = host
            .apply_action("p-back-no-to", "s1", ActionKind::Back, Vec::new())
            .await
            .unwrap_err();
        assert!(format!("{error}").contains("'to'"));

        // The effect (Backed) committed before the reentry dispatch — the
        // refusal is the missing redo target, not the action itself.
        let after = host.snapshot("p-back-no-to").await.unwrap().unwrap();
        assert_ne!(
            before.version, after.version,
            "the back effect itself is durable"
        );
    }

    /// Fail × IRQ: failing a waiting step decides it `Failed` (the legacy
    /// `Error`), skips the untouched downstream, writes the error variables,
    /// and ends the ancestor chain `Failed` (the legacy error walk).
    #[tokio::test]
    async fn fail_on_waiting_step_skips_downstream_and_decides_ancestors() {
        let workflow = crate::model::Workflow::new()
            .with_step(|step| step.with_id("s1"))
            .with_step(|step| {
                step.with_id("s2").with_uses(
                    crate::utils::test::USES_IRQ,
                    crate::Vars::new().with("key", "fail-act"),
                )
            })
            .with_step(|step| step.with_id("s3"));
        let (host, mut wake_rx) = KernelHost::in_memory();
        let spec = ProcessSpec::from_workflow("p-fail", &workflow, &|_| None).unwrap();
        host.start_process(&spec).await.unwrap();
        host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();

        host.apply_action(
            "p-fail",
            "s2",
            ActionKind::Fail {
                code: "E42".into(),
                message: "boom".into(),
            },
            Vec::new(),
        )
        .await
        .unwrap();
        host.run_until_quiescent(&mut wake_rx, 300).await.unwrap();

        let snapshot = host.snapshot("p-fail").await.unwrap().unwrap();
        assert!(host_all_terminal(&snapshot));
        let state = |task: &str| state_of(&snapshot, task);
        assert_eq!(state("s2"), crate::kernel::ExecutionState::Failed);
        assert_eq!(state("s3"), crate::kernel::ExecutionState::Skipped);
        // The completed sibling is not an ancestor; the root is.
        assert_eq!(state("s1"), crate::kernel::ExecutionState::Completed);
        assert_eq!(state("$"), crate::kernel::ExecutionState::Failed);
        let s2 = crate::kernel::ExecutionId::new("s2", 0).unwrap();
        assert_eq!(
            snapshot
                .variables
                .get(&(s2, "error.code".to_string()))
                .map(String::as_str),
            Some("E42")
        );
    }

    /// The legacy comparison for the abort matrix: the same IRQ chain, the
    /// same abort, and every legacy task (acts folded into their owning
    /// step) carries the kernel's per-node state.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn kernel_abort_matches_legacy_on_irq_chain() {
        let workflow = || {
            crate::model::Workflow::new()
                .with_step(|step| step.with_id("s1"))
                .with_step(|step| {
                    step.with_id("s2").with_uses(
                        crate::utils::test::USES_IRQ,
                        crate::Vars::new().with("key", "parity-abort"),
                    )
                })
                .with_step(|step| step.with_id("s3"))
        };

        // Legacy: abort the waiting act of s2 the moment it appears.
        let engine = crate::Engine::builder().start().await.unwrap();
        let created = {
            let sig = engine.signal((String::new(), String::new()));
            let (done, done_rx) = (sig.clone(), sig.clone());
            let channel = engine.channel();
            channel.on_message(move |e| {
                let done = done_rx.clone();
                async move {
                    if e.is_params_key("parity-abort") && e.is_state(crate::MessageState::Created) {
                        done.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                        done.close();
                    }
                }
            });
            done
        };
        let legacy_proc = engine
            .runtime()
            .start(&workflow(), crate::Vars::new())
            .await
            .unwrap();
        let (pid, tid) = tokio::time::timeout(std::time::Duration::from_secs(30), created.recv())
            .await
            .expect("the waiting act never appeared")
            .clone();
        engine
            .runtime()
            .do_action(&crate::event::Action::new(
                &pid,
                &tid,
                crate::event::EventAction::Abort,
                crate::Vars::new(),
            ))
            .await
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !legacy_proc.state().is_completed() {
            assert!(
                std::time::Instant::now() < deadline,
                "legacy never settled the aborted run"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let legacy = folded_legacy_states(&legacy_proc);

        // Kernel host: the same model, the same abort on the step.
        let (host, mut wake_rx) = KernelHost::in_memory();
        let spec = ProcessSpec::from_workflow("k-abort", &workflow(), &|_| None).unwrap();
        host.start_process(&spec).await.unwrap();
        host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
        host.apply_action("k-abort", "s2", ActionKind::Abort, Vec::new())
            .await
            .unwrap();
        host.run_until_quiescent(&mut wake_rx, 300).await.unwrap();
        let snapshot = host.snapshot("k-abort").await.unwrap().unwrap();

        assert_parity(&legacy, &snapshot);
        engine.close().await;
    }

    /// The legacy comparison for the Fail (Error) matrix.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn kernel_fail_matches_legacy_on_irq_chain() {
        let workflow = || {
            crate::model::Workflow::new()
                .with_step(|step| step.with_id("s1"))
                .with_step(|step| {
                    step.with_id("s2").with_uses(
                        crate::utils::test::USES_IRQ,
                        crate::Vars::new().with("key", "parity-fail"),
                    )
                })
                .with_step(|step| step.with_id("s3"))
        };

        let engine = crate::Engine::builder().start().await.unwrap();
        let created = {
            let sig = engine.signal((String::new(), String::new()));
            let (done, done_rx) = (sig.clone(), sig.clone());
            let channel = engine.channel();
            channel.on_message(move |e| {
                let done = done_rx.clone();
                async move {
                    if e.is_params_key("parity-fail") && e.is_state(crate::MessageState::Created) {
                        done.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                        done.close();
                    }
                }
            });
            done
        };
        let legacy_proc = engine
            .runtime()
            .start(&workflow(), crate::Vars::new())
            .await
            .unwrap();
        let (pid, tid) = tokio::time::timeout(std::time::Duration::from_secs(30), created.recv())
            .await
            .expect("the waiting act never appeared")
            .clone();
        let mut options = crate::Vars::new();
        options.set("ecode", "E42");
        options.set("message", "boom");
        engine
            .runtime()
            .do_action(&crate::event::Action::new(
                &pid,
                &tid,
                crate::event::EventAction::Error,
                options,
            ))
            .await
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !legacy_proc.state().is_completed() {
            assert!(
                std::time::Instant::now() < deadline,
                "legacy never settled the failed run"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let legacy = folded_legacy_states(&legacy_proc);

        let (host, mut wake_rx) = KernelHost::in_memory();
        let spec = ProcessSpec::from_workflow("k-fail", &workflow(), &|_| None).unwrap();
        host.start_process(&spec).await.unwrap();
        host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
        host.apply_action(
            "k-fail",
            "s2",
            ActionKind::Fail {
                code: "E42".into(),
                message: "boom".into(),
            },
            Vec::new(),
        )
        .await
        .unwrap();
        host.run_until_quiescent(&mut wake_rx, 300).await.unwrap();
        let snapshot = host.snapshot("k-fail").await.unwrap().unwrap();

        assert_parity(&legacy, &snapshot);
        engine.close().await;
    }

    /// Whether every task of a legacy run is decided (or never started): the
    /// fold below reads a state walk that may still be in flight.
    async fn engines_quiescent(engine: &crate::Engine, pid: &str) -> bool {
        match engine.runtime().proc(pid).await {
            Ok(Some(proc)) => proc.tasks().iter().all(|task| {
                let state = task.state();
                state.is_completed() || state.is_none()
            }),
            _ => true,
        }
    }

    /// The fold for a run whose nodes may carry several instances over time: a
    /// timeout branch whose window is not open yet is dispatched (and skipped)
    /// again on every legacy tick, while the kernel keeps one execution per
    /// node. So the comparison takes the *latest* instance per node — creation
    /// order, which is instance order for a node re-dispatched by the tick —
    /// with acts folded into their owning step like [`folded_legacy_states`].
    fn latest_folded_legacy_states(proc: &Arc<crate::scheduler::Process>) -> Vec<(String, String)> {
        let mut latest: std::collections::BTreeMap<String, (i64, String)> =
            std::collections::BTreeMap::new();
        for task in proc.tasks() {
            if task.state().is_none() {
                continue;
            }
            let state = match task.state() {
                crate::TaskState::Completed | crate::TaskState::Submitted => "Completed",
                crate::TaskState::Skipped => "Skipped",
                crate::TaskState::Aborted => "Aborted",
                crate::TaskState::Error => "Failed",
                crate::TaskState::Backed => "Backed",
                crate::TaskState::Cancelled => "Cancelled",
                crate::TaskState::Interrupt => "Waiting",
                other => panic!("unexpected legacy state {other:?}"),
            };
            let node = if task.node().kind() == crate::NodeKind::Workflow {
                crate::utils::consts::TASK_ROOT_TID.to_string()
            } else if task.node().kind() == crate::NodeKind::Act {
                task.parent(proc)
                    .expect("act without a step")
                    .node()
                    .id()
                    .to_string()
            } else {
                task.node().id().to_string()
            };
            let entry = latest.entry(node).or_insert((i64::MIN, state.to_string()));
            if task.timestamp >= entry.0 {
                *entry = (task.timestamp, state.to_string());
            }
        }
        latest
            .into_iter()
            .map(|(node, (_, state))| (node, state))
            .collect()
    }

    /// Run the timeout workflow on one engine the way a client would: complete
    /// the init act, watch the branch acts' messages until both windows opened,
    /// then complete the owner's act and wait for the run to settle. The branch
    /// acts' messages are the observable both engines share on this path (the
    /// legacy tick's intermediate skipped instances are its own).
    async fn run_timeout_engine(
        kernel_engine: bool,
        pid: &str,
        workflow: &crate::model::Workflow,
    ) -> (crate::Engine, Arc<crate::scheduler::Process>) {
        let mut config = crate::Config::default();
        config.data.kernel_engine = kernel_engine;
        config.data.tick_interval_secs = Some(1);
        let engine = crate::Engine::builder()
            .set_config(&config)
            .start()
            .await
            .unwrap();
        let runtime = engine.runtime().clone();
        let wait_for_created = |key: &'static str| {
            let sig = engine.signal((String::new(), String::new()));
            let (done, done_rx) = (sig.clone(), sig.clone());
            engine.channel().on_message(move |e| {
                let done = done_rx.clone();
                async move {
                    if e.is_params_key(key) && e.is_state(crate::MessageState::Created) {
                        done.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                        done.close();
                    }
                }
            });
            done
        };
        let init_created = wait_for_created("init");
        let act1_created = wait_for_created("act1");
        let fired = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
        {
            let fired = fired.clone();
            engine.channel().on_message(move |e| {
                let fired = fired.clone();
                async move {
                    for key in ["msg1", "msg2"] {
                        if !e.is_params_key(key) {
                            continue;
                        }
                        let mut fired = fired.lock();
                        if !fired.iter().any(|seen| seen == key) {
                            fired.push(key.to_string());
                        }
                    }
                }
            });
        }
        let ran = runtime
            .start(
                workflow,
                crate::Vars::new().with(crate::utils::consts::PROCESS_ID, pid),
            )
            .await
            .unwrap();
        let (pid_now, tid) =
            tokio::time::timeout(std::time::Duration::from_secs(30), init_created.recv())
                .await
                .expect("the init act never appeared")
                .clone();
        runtime
            .do_action(&crate::event::Action::new(
                &pid_now,
                &tid,
                crate::event::EventAction::Next,
                crate::Vars::new(),
            ))
            .await
            .expect("the init act must complete");
        let (pid_now, act1) =
            tokio::time::timeout(std::time::Duration::from_secs(30), act1_created.recv())
                .await
                .expect("the act1 act never appeared")
                .clone();
        // The owner waits on its act; the ticks fire the declared windows in
        // order, so the second branch only appears after its own window opened.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(12);
        while fired.lock().len() < 2 {
            if std::time::Instant::now() >= deadline {
                let dump = if kernel_engine {
                    let host = engine.runtime().kernel().expect("the kernel host").clone();
                    let snapshot = host.snapshot(pid).await.unwrap().expect("the aggregate");
                    format!(
                        "executions={:?} operations={:?} owner_vars={:?}",
                        snapshot
                            .executions
                            .iter()
                            .map(|(id, execution)| format!("{id:?}:{:?}", execution.state))
                            .collect::<Vec<_>>(),
                        snapshot
                            .operations
                            .values()
                            .map(|operation| format!(
                                "{:?}:{:?}:{:?}",
                                operation.id, operation.kind, operation.state
                            ))
                            .collect::<Vec<_>>(),
                        snapshot
                            .variables
                            .iter()
                            .filter(|((scope, _), _)| scope.task_id == "s1")
                            .map(|((_, name), value)| format!("{name}={value}"))
                            .collect::<Vec<_>>(),
                    )
                } else {
                    format!(
                        "tasks={:?}",
                        ran.tasks()
                            .iter()
                            .map(|task| format!(
                                "{}:{}:{}",
                                task.node().id(),
                                task.node().kind(),
                                task.state()
                            ))
                            .collect::<Vec<_>>()
                    )
                };
                panic!(
                    "the timeout branches never fired: {:?}; kernel_engine={kernel_engine}; {dump}",
                    fired.lock()
                );
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        // Both branches fired and settled: the owner's act completes, and the
        // step with it.
        runtime
            .do_action(&crate::event::Action::new(
                &pid_now,
                &act1,
                crate::event::EventAction::Next,
                crate::Vars::new(),
            ))
            .await
            .expect("the act1 act must complete");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let settled = if kernel_engine {
                engine
                    .runtime()
                    .kernel()
                    .expect("the kernel host")
                    .process_completed(pid)
                    .await
                    .unwrap()
            } else {
                ran.state().is_completed()
            };
            if settled {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the run never settled"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        (engine, ran)
    }

    /// The timeout branches of a step fire from the tick, one declaration-ordered
    /// round at a time, their window the cost since the step started — both
    /// engines, end to end. The second window opens a tick after the first, and
    /// the step completes only once its act is done *and* the branches it fired
    /// settled, which is what lets the run end.
    ///
    /// The legacy side is folded by latest instance (see
    /// [`latest_folded_legacy_states`]): the legacy tick re-dispatches a branch
    /// whose window is not open yet, so the node has a skipped instance before
    /// the fired one, and the kernel's per-node read model has no counterpart.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn kernel_timeout_branches_match_the_legacy_run() {
        const PID: &str = "p_timeout_parity";
        let workflow = || {
            crate::model::Workflow::new()
                .with_id("wf-timeout")
                .with_step(|step| {
                    step.with_id("s0").with_uses(
                        crate::utils::test::USES_IRQ,
                        crate::Vars::new().with("key", "init"),
                    )
                })
                .with_step(|step| {
                    step.with_id("s1")
                        .with_uses(
                            crate::utils::test::USES_IRQ,
                            crate::Vars::new().with("key", "act1"),
                        )
                        .with_timeout(|timeout| {
                            timeout.with_id("t1").with_if("$cost() >= 300").with_uses(
                                crate::utils::test::USES_MSG,
                                crate::Vars::new().with("key", "msg1"),
                            )
                        })
                        .with_timeout(|timeout| {
                            timeout.with_id("t2").with_if("$cost() >= 1500").with_uses(
                                crate::utils::test::USES_MSG,
                                crate::Vars::new().with("key", "msg2"),
                            )
                        })
                })
                .with_step(|step| step.with_id("s2"))
        };

        let (legacy_engine, legacy) = run_timeout_engine(false, PID, &workflow()).await;
        let legacy_fold = latest_folded_legacy_states(&legacy);
        let (kernel_engine, _ran) = run_timeout_engine(true, PID, &workflow()).await;
        let host = kernel_engine
            .runtime()
            .kernel()
            .expect("the kernel host")
            .clone();
        let snapshot = host.snapshot(PID).await.unwrap().expect("the aggregate");

        for node in ["t1", "t2"] {
            assert_eq!(
                legacy_fold
                    .iter()
                    .find(|(name, _)| name == node)
                    .map(|(_, state)| state.as_str()),
                Some("Completed"),
                "the legacy branch '{node}' must have fired and settled"
            );
            assert_eq!(
                state_of(&snapshot, node),
                crate::kernel::ExecutionState::Completed,
                "the kernel branch '{node}' must have fired and settled"
            );
        }
        // Every node either engine decided carries the same state, the kernel's
        // extras (the pre-created chain) included.
        assert_parity(&legacy_fold, &snapshot);
        kernel_engine.close().await;
        legacy_engine.close().await;
    }

    /// Every legacy task, folded onto the kernel's node vocabulary: acts
    /// fold into their owning step, the workflow root becomes `$`, and the
    /// state names map to the kernel's (Interrupt→Waiting, Error→Failed).
    ///
    /// The fold reads the live process the run returned, not the rows: a
    /// settled run's rows are sweeper-owned and may already be gone.
    fn folded_legacy_states(proc: &Arc<crate::scheduler::Process>) -> Vec<(String, String)> {
        let mut folded: Vec<(String, String)> = proc
            .tasks()
            .iter()
            .filter_map(|task| {
                // A task that never started decided no node: the legacy engine
                // leaves the inner steps of a branch it never entered behind
                // (`None`), and the kernel never creates them at all.
                if task.state().is_none() {
                    return None;
                }
                let state = match task.state() {
                    crate::TaskState::Completed | crate::TaskState::Submitted => "Completed",
                    crate::TaskState::Skipped => "Skipped",
                    crate::TaskState::Aborted => "Aborted",
                    crate::TaskState::Error => "Failed",
                    crate::TaskState::Backed => "Backed",
                    other => panic!("unexpected legacy terminal state {other:?}"),
                };
                let node = if task.node().kind() == crate::NodeKind::Workflow {
                    crate::utils::consts::TASK_ROOT_TID.to_string()
                } else if task.node().kind() == crate::NodeKind::Act {
                    task.parent(proc)
                        .expect("act without a step")
                        .node()
                        .id()
                        .to_string()
                } else {
                    task.node().id().to_string()
                };
                Some((node, state.to_string()))
            })
            .collect();
        folded.sort();
        folded.dedup();
        folded
    }

    // ---- the effect seam: Func packages run inside the ApplyAction hop ----

    /// Every `uses` step writes a durable `message.created` record when it
    /// starts — the legacy act's Created message: the Irq step waits on it
    /// (Waiting), the Func step pairs it with its completion.
    #[tokio::test]
    async fn uses_steps_write_created_message_records() {
        // A registry with one Func and one Irq package, so the run_as
        // decisions match the shapes under test.
        struct LocalFuncPackage;
        #[async_trait::async_trait]
        impl crate::package::ActPackage for LocalFuncPackage {
            fn new(_: &crate::Config) -> crate::Result<Self> {
                Ok(Self)
            }
            fn definition() -> crate::package::ActPackageDefinition {
                crate::package::ActPackageDefinition {
                    id: "test.func",
                    name: "Func",
                    desc: "",
                    icon: "",
                    doc: "",
                    version: "0.1.0",
                    schema: serde_json::json!({}),
                    options: None,
                    run_as: crate::package::ActRunAs::Func,
                    resources: Vec::new(),
                    catalog: crate::package::ActPackageCatalog::App,
                }
            }
        }
        struct LocalIrqPackage;
        #[async_trait::async_trait]
        impl crate::package::ActPackage for LocalIrqPackage {
            fn new(_: &crate::Config) -> crate::Result<Self> {
                Ok(Self)
            }
            fn definition() -> crate::package::ActPackageDefinition {
                crate::package::ActPackageDefinition {
                    id: "test.irq",
                    name: "Irq",
                    desc: "",
                    icon: "",
                    doc: "",
                    version: "0.1.0",
                    schema: serde_json::json!({}),
                    options: None,
                    run_as: crate::package::ActRunAs::Irq,
                    resources: Vec::new(),
                    catalog: crate::package::ActPackageCatalog::App,
                }
            }
        }
        let package = crate::package::Package::default();
        package.register(
            "test.func",
            &crate::package::ActPackageRegister::new::<LocalFuncPackage>(),
        );
        package.register(
            "test.irq",
            &crate::package::ActPackageRegister::new::<LocalIrqPackage>(),
        );
        let run_as = move |id: &str| package.get(id).map(|register| (register.meta)().run_as);

        let (host, mut wake_rx) = KernelHost::in_memory();
        let workflow = crate::model::Workflow::new()
            .with_step(|step| {
                step.with_id("s1")
                    .with_uses("test.func", crate::Vars::new().with("k1", "v1"))
            })
            .with_step(|step| {
                step.with_id("s2")
                    .with_uses("test.irq", crate::Vars::new().with("key", "irq-1"))
            });
        let spec = ProcessSpec::from_workflow("p-msg", &workflow, &run_as).unwrap();
        host.start_process(&spec).await.unwrap();
        host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();

        let snapshot = host.snapshot("p-msg").await.unwrap().unwrap();
        let message_of = |nid: &str| {
            snapshot
                .outbox
                .values()
                .find(|record| {
                    record.topic == "message.created"
                        && serde_json::from_str::<serde_json::Value>(&record.payload)
                            .ok()
                            .and_then(|payload| {
                                payload
                                    .get("nid")
                                    .and_then(|nid| nid.as_str())
                                    .map(String::from)
                            })
                            .as_deref()
                            == Some(nid)
                })
                .unwrap_or_else(|| panic!("no message.created record for '{nid}'"))
        };
        let s1_message = message_of("s1");
        let s1_payload: serde_json::Value = serde_json::from_str(&s1_message.payload).unwrap();
        assert_eq!(s1_payload["uses"], "test.func");
        assert_eq!(s1_payload["inputs"]["params"]["k1"], "v1");
        let s2_message = message_of("s2");
        let s2_payload: serde_json::Value = serde_json::from_str(&s2_message.payload).unwrap();
        assert_eq!(s2_payload["uses"], "test.irq");
        assert_eq!(s2_payload["inputs"]["params"]["key"], "irq-1");

        // The Irq execution is parked on its message; the plain leaf behind
        // it waits for the client.
        let state = |task: &str| state_of(&snapshot, task);
        assert_eq!(state("s2"), crate::kernel::ExecutionState::Waiting);
        assert_eq!(state("s1"), crate::kernel::ExecutionState::Completed);

        // The records are deliverable: a transport sees them like any other
        // outbox record.
        struct TopicTransport {
            topics: Arc<parking_lot::Mutex<Vec<String>>>,
        }
        impl crate::kernel::OutboxTransport for TopicTransport {
            fn deliver(&mut self, record: &OutboxRecord) -> DeliveryOutcome {
                self.topics.lock().push(record.topic.clone());
                DeliveryOutcome::Delivered {
                    receipt: "ok".into(),
                }
            }
        }
        let topics = Arc::new(parking_lot::Mutex::new(Vec::new()));
        host.attach_outbox_transport(Box::new(TopicTransport {
            topics: topics.clone(),
        }));
        host.dispatch_outbox().await.unwrap();
        let delivered = topics.lock().clone();
        assert_eq!(
            delivered
                .iter()
                .filter(|topic| *topic == "message.created")
                .count(),
            2,
            "both uses steps announced themselves: {delivered:?}"
        );
        // A second pass delivers nothing: every record was confirmed.
        let redelivered = host.dispatch_outbox().await.unwrap();
        assert_eq!(redelivered, 0, "delivered records are not redelivered");
    }

    /// Effect params resolve `$`-expressions against the acting execution's
    /// scope variables, exactly like a legacy task's params.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn effect_params_resolve_scope_variables() {
        let engine = crate::Engine::builder().start().await.unwrap();
        let (host, mut wake_rx) = KernelHost::in_memory();
        host.attach_effect_runner(Arc::new(effects::PackageEffectRunner::new(
            engine.runtime().clone(),
        )));
        let package = engine.runtime().package().clone();
        let run_as = move |id: &str| package.get(id).map(|register| (register.meta)().run_as);

        let workflow = crate::model::Workflow::new()
            .with_var("v", 5)
            .with_step(|step| {
                step.with_id("s1").with_uses(
                    crate::utils::test::USES_SET,
                    crate::Vars::new().with("n", "${{ v }}"),
                )
            });
        let spec = ProcessSpec::from_workflow("p-params", &workflow, &run_as).unwrap();
        host.start_process(&spec).await.unwrap();
        host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();

        let snapshot = host.snapshot("p-params").await.unwrap().unwrap();
        let s1 = crate::kernel::ExecutionId::new("s1", 0).unwrap();
        assert_eq!(
            snapshot
                .variables
                .get(&(s1.clone(), "n".to_string()))
                .map(String::as_str),
            Some("5"),
            "the params expression must resolve the workflow variable"
        );
        engine.close().await;
    }

    /// The whole message path: a Func step's package runs, its completion
    /// record and the Irq step's Created record both forward through
    /// [`message::MessageForwarder`] as engine messages.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn outbox_records_forward_as_engine_messages() {
        let engine = crate::Engine::builder().start().await.unwrap();
        let (host, mut wake_rx) = KernelHost::in_memory();
        host.attach_effect_runner(Arc::new(effects::PackageEffectRunner::new(
            engine.runtime().clone(),
        )));
        let collected: Arc<parking_lot::Mutex<Vec<crate::Message>>> =
            Arc::new(parking_lot::Mutex::new(Vec::new()));
        let sink = collected.clone();
        host.attach_outbox_transport(Box::new(message::MessageForwarder::new(
            move |msg: &crate::Message| sink.lock().push(msg.clone()),
        )));
        let package = engine.runtime().package().clone();
        let run_as = move |id: &str| package.get(id).map(|register| (register.meta)().run_as);

        let workflow = crate::model::Workflow::new()
            .with_id("wf-msg-e2e")
            .with_step(|step| {
                step.with_id("s1").with_uses(
                    crate::utils::test::USES_SET,
                    crate::Vars::new().with("k1", "v1").with("n", 2),
                )
            })
            .with_step(|step| {
                step.with_id("s2").with_uses(
                    crate::utils::test::USES_IRQ,
                    crate::Vars::new().with("key", "irq-1"),
                )
            });
        let spec = ProcessSpec::from_workflow("p-msg-e2e", &workflow, &run_as).unwrap();
        host.start_process(&spec).await.unwrap();
        host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
        host.dispatch_outbox().await.unwrap();

        let messages = collected.lock().clone();
        let created = messages
            .iter()
            .find(|msg| msg.is_irq() && msg.state == crate::MessageState::Created)
            .expect("the Irq step's Created message must be forwarded");
        assert_eq!(created.pid, "p-msg-e2e");
        assert_eq!(created.tid, "s2");
        assert_eq!(created.mid, "wf-msg-e2e");
        assert_eq!(
            created.inputs.get::<serde_json::Value>("params").unwrap()["key"],
            "irq-1"
        );
        let completed = messages
            .iter()
            .find(|msg| msg.state == crate::MessageState::Completed && msg.nid == "s1")
            .expect("the Func step's Completed message must be forwarded");
        assert_eq!(completed.outputs.get::<i32>("n"), Some(2));
        // The event channels key on the act's params key, exactly like the
        // legacy Created message.
        assert!(collect_subscriber_sees(&messages, "irq-1"));
        engine.close().await;
    }

    /// The forwarded messages are what a channel subscriber would filter on:
    /// `is_params_key` reads the params on the act's Created message.
    fn collect_subscriber_sees(messages: &[crate::Message], params_key: &str) -> bool {
        messages.iter().any(|msg| {
            msg.state == crate::MessageState::Created
                && msg
                    .inputs
                    .get::<serde_json::Value>("params")
                    .and_then(|params| {
                        params
                            .get("key")
                            .and_then(|value| value.as_str())
                            .map(String::from)
                    })
                    .as_deref()
                    == Some(params_key)
        })
    }

    /// The parity contract for decision walks: every node legacy decided
    /// carries exactly the kernel's state, and the kernel's extra nodes (the
    /// pre-created chain legacy never reached) are all terminal.
    fn assert_parity(legacy: &[(String, String)], snapshot: &Snapshot) {
        for (node, state) in legacy {
            let kernel_state = format!("{:?}", state_of(snapshot, node));
            assert_eq!(&kernel_state, state, "node '{node}' diverged");
        }
        for execution in snapshot.executions.values() {
            let decided = legacy
                .iter()
                .any(|(node, _)| execution.model_node.as_str() == node);
            assert!(
                !decided || execution.state.is_terminal(),
                "kernel node '{}' is not terminal: {:?}",
                execution.model_node.as_str(),
                execution.state
            );
        }
    }

    /// A failing Func package, registered per-test.
    struct FailingPackage;

    #[async_trait::async_trait]
    impl crate::package::ActPackage for FailingPackage {
        fn new(_: &crate::Config) -> crate::Result<Self> {
            Ok(Self)
        }
        fn definition() -> crate::package::ActPackageDefinition {
            crate::package::ActPackageDefinition {
                id: "test.fail",
                name: "Fail",
                desc: "always errors",
                icon: "",
                doc: "",
                version: "0.1.0",
                schema: serde_json::json!({}),
                options: None,
                run_as: crate::package::ActRunAs::Func,
                resources: Vec::new(),
                catalog: crate::package::ActPackageCatalog::App,
            }
        }
        async fn execute(
            &self,
            _ctx: &crate::Context,
            _params: &serde_json::Value,
        ) -> crate::Result<Option<crate::Vars>> {
            Err(crate::ActError::Package("boom".into()))
        }
    }

    /// A Func step's package runs inside its completion hop: the outputs land
    /// on the acting execution's scope in the same atomic commit.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn func_package_runs_inside_the_completion_hop() {
        let engine = crate::Engine::builder().start().await.unwrap();
        let (host, mut wake_rx) = KernelHost::in_memory();
        host.attach_effect_runner(Arc::new(effects::PackageEffectRunner::new(
            engine.runtime().clone(),
        )));
        // The auto/waiting decision reads the same registry the engine does.
        let package = engine.runtime().package().clone();
        let run_as = move |id: &str| package.get(id).map(|register| (register.meta)().run_as);

        let workflow = crate::model::Workflow::new()
            .with_step(|step| {
                step.with_id("s1").with_uses(
                    crate::utils::test::USES_SET,
                    crate::Vars::new().with("k1", "v1").with("n", 2),
                )
            })
            .with_step(|step| step.with_id("s2"));
        let spec = ProcessSpec::from_workflow("p-effect", &workflow, &run_as).unwrap();
        host.start_process(&spec).await.unwrap();
        let report = host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
        assert_eq!(report.retried, 0, "the effect must land: {report:?}");
        assert!(host.process_completed("p-effect").await.unwrap());

        let snapshot = host.snapshot("p-effect").await.unwrap().unwrap();
        let s1 = crate::kernel::ExecutionId::new("s1", 0).unwrap();
        assert_eq!(
            snapshot
                .variables
                .get(&(s1.clone(), "k1".to_string()))
                .map(String::as_str),
            Some("\"v1\""),
            "the package outputs are scope variables of the acting execution"
        );
        assert_eq!(
            snapshot
                .variables
                .get(&(s1.clone(), "n".to_string()))
                .map(String::as_str),
            Some("2")
        );
        let state = |task: &str| state_of(&snapshot, task);
        assert_eq!(state("s1"), crate::kernel::ExecutionState::Completed);
        assert_eq!(state("s2"), crate::kernel::ExecutionState::Completed);
        engine.close().await;
    }

    /// A package error decides the execution Failed with the error variables
    /// and the error walk takes over (upstream Failed, downstream Skipped).
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn package_error_fails_the_execution_and_walks_up() {
        let engine = crate::Engine::builder().start().await.unwrap();
        engine.runtime().package().register(
            "test.fail",
            &crate::package::ActPackageRegister::new::<FailingPackage>(),
        );
        // The effect runner resolves the package through its durable metadata
        // row (like the legacy engine's `package_definition`), so the failure
        // under test is the package's own.
        engine
            .executor(&crate::Principal::unrestricted())
            .ext()
            .register_package(&<FailingPackage as crate::package::ActPackage>::definition())
            .await
            .unwrap();
        let (host, mut wake_rx) = KernelHost::in_memory();
        host.attach_effect_runner(Arc::new(effects::PackageEffectRunner::new(
            engine.runtime().clone(),
        )));
        let package = engine.runtime().package().clone();
        let run_as = move |id: &str| package.get(id).map(|register| (register.meta)().run_as);

        let workflow = crate::model::Workflow::new()
            .with_step(|step| step.with_id("s0"))
            .with_step(|step| {
                step.with_id("s1")
                    .with_uses("test.fail", crate::Vars::new())
            })
            .with_step(|step| step.with_id("s2"));
        let spec = ProcessSpec::from_workflow("p-effect-fail", &workflow, &run_as).unwrap();
        host.start_process(&spec).await.unwrap();
        host.run_until_quiescent(&mut wake_rx, 300).await.unwrap();

        assert!(host.process_completed("p-effect-fail").await.unwrap());
        let snapshot = host.snapshot("p-effect-fail").await.unwrap().unwrap();
        let s1 = crate::kernel::ExecutionId::new("s1", 0).unwrap();
        assert_eq!(
            snapshot
                .variables
                .get(&(s1.clone(), "error.code".to_string()))
                .map(String::as_str),
            Some("EPACKAGE")
        );
        assert!(
            snapshot
                .variables
                .contains_key(&(s1.clone(), "error.message".to_string()))
        );
        let state = |task: &str| state_of(&snapshot, task);
        assert_eq!(state("s1"), crate::kernel::ExecutionState::Failed);
        // The error walk: the ancestor chain ends Failed, the untouched
        // downstream skips.
        assert_eq!(state("$"), crate::kernel::ExecutionState::Failed);
        assert_eq!(state("s0"), crate::kernel::ExecutionState::Completed);
        assert_eq!(state("s2"), crate::kernel::ExecutionState::Skipped);
        engine.close().await;
    }

    /// `scope_vars` is the variable space an effect's params resolve against:
    /// the chain of the acting execution, with the engine's own aggregate
    /// facts (`__model`, `__auto`, `__owner`) left out — a legacy task never
    /// reads them.
    #[test]
    fn scope_vars_carry_the_run_variables_without_the_engine_facts() {
        let workflow = crate::model::Workflow::new()
            .with_var("v", 5)
            .with_step(|step| step.with_id("s1"));
        let spec = ProcessSpec::from_workflow("p-vars", &workflow, &|_| None).unwrap();
        let snapshot = spec.seed_snapshot().unwrap();
        let s1 = crate::kernel::ExecutionId::new("s1", 0).unwrap();
        let names: Vec<String> = scope_vars(&snapshot, &s1)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert!(names.contains(&"v".to_string()), "{names:?}");
        assert!(
            !names.iter().any(|name| name.starts_with("__")),
            "engine facts must not be variables: {names:?}"
        );
    }

    /// Effect params seal snapshot data through the legacy task-init path: the
    /// headless scope carries the run's owner, derives the target's scope key
    /// from the acting execution's variables, seals under the owner's
    /// authority, and only then fills `${{ }}`. The legacy engine, run on the
    /// same store with the same owner and snapshot, hands its package the same
    /// params — the parity the shadow exists to prove.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn effect_seals_snapshot_params_like_the_legacy_run() {
        let engine = crate::Engine::builder().start().await.unwrap();
        engine
            .add_snapshot(
                "profile",
                crate::SnapshotOptions {
                    scope: vec!["uid".to_string()],
                    ..crate::SnapshotOptions::per_task()
                },
            )
            .unwrap();
        engine
            .snapshot()
            .upsert(
                "profile",
                "u1",
                1,
                crate::Vars::new().with("secrets", crate::Vars::new().with("TOKEN", "sk-1")),
            )
            .unwrap();
        let owner = crate::ScopePolicy {
            subject: "u1".to_string(),
            scopes: std::collections::HashMap::from([(
                "profile".to_string(),
                vec!["$subject".to_string()],
            )]),
            ..crate::ScopePolicy::deny_all()
        };

        // The scope key comes from the run's start inputs, the way a real
        // `proc:start` hands its `uid` in — not from a declared workflow var.
        let start_vars = crate::Vars::new().with("uid", "u1");
        let workflow = crate::model::Workflow::new().with_step(|step| {
            step.with_id("s1").with_uses(
                crate::utils::test::USES_SET,
                crate::Vars::new()
                    .with("token", "${{ $profile.secrets.TOKEN }}")
                    .with("uid", "${{ uid }}"),
            )
        });

        // The kernel side: the effect runs the package inside the completion
        // hop, and its outputs (the resolved params) land on the execution.
        let (host, mut wake_rx) = KernelHost::in_memory();
        host.attach_effect_runner(Arc::new(effects::PackageEffectRunner::new(
            engine.runtime().clone(),
        )));
        let package = engine.runtime().package().clone();
        let run_as = move |id: &str| package.get(id).map(|register| (register.meta)().run_as);
        let spec = ProcessSpec::from_workflow("p-seal", &workflow, &run_as)
            .unwrap()
            .with_owner(owner.clone())
            .with_vars(&start_vars);
        host.start_process(&spec).await.unwrap();
        let report = host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();
        assert_eq!(report.retried, 0, "the seal must land: {report:?}");
        assert!(host.process_completed("p-seal").await.unwrap());
        let snapshot = host.snapshot("p-seal").await.unwrap().unwrap();
        let s1 = crate::kernel::ExecutionId::new("s1", 0).unwrap();
        let kernel = |name: &str| {
            snapshot
                .variables
                .get(&(s1.clone(), name.to_string()))
                .unwrap_or_else(|| panic!("effect output '{name}' missing"))
                .clone()
        };
        let kernel_token = kernel("token");
        let kernel_uid = kernel("uid");

        // The legacy side: the same workflow, owner and snapshot on the same
        // engine. Its step dispatches the act with the resolved params.
        let (sig, rx) = engine.signal(()).double();
        crate::utils::test::auto_complete(&engine, &rx);
        let proc = engine
            .runtime()
            .start(
                &workflow,
                start_vars
                    .clone()
                    .with(crate::utils::consts::PROC_OWNER, owner.clone()),
            )
            .await
            .unwrap();
        sig.recv().await;
        let legacy = proc
            .task_by_uses(crate::utils::test::USES_SET)
            .last()
            .cloned()
            .expect("the legacy act task ran");

        // The aggregate stores the JSON encoding of what the legacy package
        // was handed.
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&kernel_token).unwrap(),
            legacy.data().get::<serde_json::Value>("token").unwrap()
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&kernel_uid).unwrap(),
            legacy.data().get::<serde_json::Value>("uid").unwrap()
        );
        assert_eq!(
            legacy.data().get::<serde_json::Value>("token").unwrap(),
            serde_json::json!("sk-1"),
            "the sealed snapshot value must be what the package received"
        );
        engine.close().await;
    }

    /// The seal is the owner's: a run whose authority does not own the scope
    /// reads nothing and its effect fails where the legacy task would, rather
    /// than leaking another subject's data.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn effect_denies_a_snapshot_scope_the_owner_lacks() {
        let engine = crate::Engine::builder().start().await.unwrap();
        engine
            .add_snapshot(
                "profile",
                crate::SnapshotOptions {
                    scope: vec!["uid".to_string()],
                    ..crate::SnapshotOptions::per_task()
                },
            )
            .unwrap();
        engine
            .snapshot()
            .upsert(
                "profile",
                "u1",
                1,
                crate::Vars::new().with("secrets", crate::Vars::new().with("TOKEN", "sk-1")),
            )
            .unwrap();
        // The pattern names its own subject, which is not the scope key.
        let intruder = crate::ScopePolicy {
            subject: "intruder".to_string(),
            scopes: std::collections::HashMap::from([(
                "profile".to_string(),
                vec!["$subject".to_string()],
            )]),
            ..crate::ScopePolicy::deny_all()
        };

        // The scope key resolves from the run's start inputs (see the parity
        // test), so the seal is what the authority fails on — not a missing
        // key.
        let workflow = crate::model::Workflow::new().with_step(|step| {
            step.with_id("s1").with_uses(
                crate::utils::test::USES_SET,
                crate::Vars::new().with("token", "${{ $profile.secrets.TOKEN }}"),
            )
        });

        let (host, mut wake_rx) = KernelHost::in_memory();
        host.attach_effect_runner(Arc::new(effects::PackageEffectRunner::new(
            engine.runtime().clone(),
        )));
        let package = engine.runtime().package().clone();
        let run_as = move |id: &str| package.get(id).map(|register| (register.meta)().run_as);
        let spec = ProcessSpec::from_workflow("p-denied", &workflow, &run_as)
            .unwrap()
            .with_owner(intruder)
            .with_vars(&crate::Vars::new().with("uid", "u1"));
        host.start_process(&spec).await.unwrap();
        host.run_until_quiescent(&mut wake_rx, 200).await.unwrap();

        assert!(host.process_completed("p-denied").await.unwrap());
        let snapshot = host.snapshot("p-denied").await.unwrap().unwrap();
        let s1 = crate::kernel::ExecutionId::new("s1", 0).unwrap();
        assert_eq!(
            state_of(&snapshot, "s1"),
            crate::kernel::ExecutionState::Failed
        );
        let error = snapshot
            .variables
            .get(&(s1.clone(), "error.message".to_string()))
            .expect("the denied seal fails the effect")
            .clone();
        assert!(error.contains("not owned"), "unexpected error: {error}");
        assert!(
            !snapshot.variables.contains_key(&(s1, "token".to_string())),
            "a denied seal must not produce outputs"
        );
        engine.close().await;
    }
}
