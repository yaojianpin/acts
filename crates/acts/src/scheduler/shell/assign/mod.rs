//! A human act's assignment: the declared owner, its persisted task state,
//! and the per-task decision authorization.
//!
//! The declaration (`Step::assign`) is resolved at act start through the same
//! headless scope and `fill_params` path the act's params use. The resolved
//! `__assignment` is a private task variable — the only authorization source —
//! and the message fields are its projection. An act that declares `assign`
//! without resolving to an owner fails at start: the engine never produces an
//! unowned approval.

use super::*;

/// The private execution variable carrying one human act's assignment:
/// `{"owner","assignee","rev","transferred_from","updated_by",
/// "updated_at"}`. It is the authorization source; the message fields are
/// only its projection.
pub(super) const ASSIGNMENT_VAR: &str = "__assignment";

/// The assignment state of one human decision execution.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub(super) struct Assignment {
    pub(super) owner: String,
    pub(super) assignee: String,
    #[serde(default)]
    pub(super) rev: u64,
    #[serde(default)]
    pub(super) transferred_from: Option<String>,
    #[serde(default)]
    pub(super) updated_by: Option<String>,
    #[serde(default)]
    pub(super) updated_at: i64,
}

impl Assignment {
    /// The initial assignment a declared `assign.owner` produces.
    pub(super) fn new(owner: &str) -> Self {
        Self {
            owner: owner.to_string(),
            assignee: owner.to_string(),
            rev: 0,
            transferred_from: None,
            updated_by: None,
            updated_at: 0,
        }
    }

    /// The aggregate's JSON-text representation (a scope variable value).
    pub(super) fn value(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// The assignment a transfer to `to` produces: the target becomes both
    /// owner and assignee, the revision advances, and the old owner and the
    /// caller are recorded for audit.
    pub(super) fn transferred(&self, to: &str, by: Option<&str>, now: i64) -> Self {
        Self {
            owner: to.to_string(),
            assignee: to.to_string(),
            rev: self.rev.saturating_add(1),
            transferred_from: Some(self.owner.clone()),
            updated_by: by.map(str::to_string),
            updated_at: now,
        }
    }

    /// Read the assignment of one execution, when the act declares one.
    pub(super) fn of(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        task: &crate::scheduler::core::model::task_graph::TaskId,
    ) -> Result<Option<Self>> {
        let Some(raw) = task_variable(snapshot, task, ASSIGNMENT_VAR) else {
            return Ok(None);
        };
        serde_json::from_str(raw).map(Some).map_err(runtime)
    }
}

/// The per-task decision check: a declared assignment admits only its owner
/// or assignee, and a decision carrying a revision must name the current one.
/// No assignment (or no authenticated subject — ACL disabled) keeps the old
/// `@execute` behavior.
pub(super) fn authorize_decision(
    assignment: Option<&Assignment>,
    subject: Option<&str>,
    rev: Option<u64>,
    node: &str,
) -> Result<()> {
    let (Some(assignment), Some(subject)) = (assignment, subject) else {
        return Ok(());
    };
    if subject != assignment.owner && subject != assignment.assignee {
        return Err(crate::ActError::Action(format!(
            "ENOTOWNER: user '{subject}' is not the owner or assignee of act '{node}'"
        )));
    }
    if let Some(rev) = rev
        && rev != assignment.rev
    {
        return Err(crate::ActError::Action(format!(
            "EASSIGNMENT_CHANGED: act '{node}' assignment rev is {}, not {rev}",
            assignment.rev
        )));
    }
    Ok(())
}

/// The per-task transfer check: the current owner, or a caller holding the
/// `assign` capability. A task without an assignment has no owner to move,
/// so an authenticated caller is refused rather than silently allowed. ACL
/// disabled (no subject) keeps the old behavior; the plan itself still
/// refuses an unassigned act.
pub(super) fn authorize_transfer(
    assignment: Option<&Assignment>,
    subject: Option<&str>,
    can_assign: bool,
    node: &str,
) -> Result<()> {
    let Some(subject) = subject else {
        return Ok(());
    };
    let Some(assignment) = assignment else {
        return Err(crate::ActError::Action(format!(
            "ENOTOWNER: act '{node}' has no assignment to transfer"
        )));
    };
    if subject == assignment.owner || can_assign {
        return Ok(());
    }
    Err(crate::ActError::Action(format!(
        "ENOTOWNER: user '{subject}' is not the owner of act '{node}' and holds no assign capability"
    )))
}

/// The caller-correctable checks a transfer must pass before any durable
/// intent exists: the act is assigned and non-terminal, the caller owns it (or
/// holds the capability), the target is named, and the client's `expect_rev`
/// — when present — names the current revision. The plan repeats them for a
/// replay; a refusal here consumes nothing.
///
/// The target is a *business* user, not an access-control user: the engine
/// implements no business user directory (a design non-goal), so `enabled`
/// and the target's own decision permission are the calling system's checks.
/// What the engine can prove it does: the target becomes the act's owner, and
/// the act's per-task authorization then admits that owner.
pub(super) fn validate_transfer(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    task_id: &crate::scheduler::core::model::task_graph::TaskId,
    state: crate::scheduler::core::model::task_graph::TaskState,
    inputs: &[(String, String)],
    subject: Option<&str>,
    can_assign: bool,
) -> Result<()> {
    let node = task_id.node.as_str();
    let assignment = Assignment::of(snapshot, task_id)?.ok_or_else(|| {
        crate::ActError::Action(format!(
            "ENOTOWNER: act '{node}' has no assignment to transfer"
        ))
    })?;
    authorize_transfer(Some(&assignment), subject, can_assign, node)?;
    if state.is_terminal() {
        return Err(crate::ActError::Action(format!(
            "task('{node}') is not allowed to transfer"
        )));
    }
    let input = |name: &str| {
        inputs
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    let to = input("to")
        .map(str::trim)
        .filter(|to| !to.is_empty())
        .ok_or_else(|| crate::ActError::Action("cannot find 'to' in act:transfer".to_string()))?;
    let _ = to;
    if let Some(raw) = input("expect_rev") {
        let expect = parse_expect_rev(raw)?;
        if expect != assignment.rev {
            return Err(crate::ActError::Action(format!(
                "EASSIGNMENT_CHANGED: act '{node}' assignment rev is {}, not {expect}",
                assignment.rev
            )));
        }
    }
    Ok(())
}

/// Parse a transfer's `expect_rev` (a JSON number or numeric string, the
/// shapes the transports carry). A malformed value is refused like a stale
/// revision: it can never authorize a transfer.
pub(super) fn parse_expect_rev(raw: &str) -> Result<u64> {
    let raw = raw.trim();
    raw.parse::<u64>().or_else(|_| {
        serde_json::from_str::<serde_json::Value>(raw)
            .ok()
            .and_then(|value| match value {
                serde_json::Value::Number(number) => number.as_u64(),
                serde_json::Value::String(text) => text.parse().ok(),
                _ => None,
            })
            .ok_or_else(|| {
                crate::ActError::Action(format!("EASSIGNMENT_CHANGED: invalid expect_rev '{raw}'"))
            })
    })
}

/// The durable id of one act's `Created` message, read from the record the
/// start commit wrote. The id lives in the outbox payload, so it survives a
/// restart and the transfer notification can reference the original message
/// without inventing one; a record from an older kernel has none and yields
/// `None`.
pub(super) fn created_message_id(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    task_id: &crate::scheduler::core::model::task_graph::TaskId,
) -> Option<String> {
    let key = format!(
        "message.created:{}:{}:{}",
        snapshot.process_id,
        task_id.node.as_str(),
        task_id.generation
    );
    snapshot
        .outbox
        .values()
        .find(|record| record.id.key == key)
        .and_then(|record| serde_json::from_str::<serde_json::Value>(&record.payload).ok())
        .and_then(|payload| {
            payload
                .get("msg_id")
                .and_then(|value| value.as_str())
                .map(str::to_string)
        })
}

/// The `assign.owner` expression one act declares, after refusing the
/// declaration where it cannot mean an owner: a package-less node, a
/// `block`/`parallel`/`sequence` container, a `Func`/`Msg` leaf, or the
/// waiting subflow. Only a human decision leaf (`ActRunAs::Irq`) may carry
/// one. The refusal names the node.
pub(super) fn declared_expression(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    task_id: &crate::scheduler::core::model::task_graph::TaskId,
    model_node: &crate::scheduler::core::model::model_graph::NodeId,
) -> Result<Option<String>> {
    let Some(model) = model_of(snapshot)? else {
        return Ok(None);
    };
    let Some(step) = find_step_in_model(&model, model_node.as_str()) else {
        // A pushed act is not in the immutable model: its declaration rides
        // the execution's durable `__act` fact.
        return pushed_declared_expression(snapshot, task_id);
    };
    let Some(assign) = &step.assign else {
        return Ok(None);
    };
    let node = task_id.node.as_str();
    let uses = step
        .uses
        .as_deref()
        .map(str::trim)
        .filter(|uses| !uses.is_empty())
        .ok_or_else(|| {
            crate::ActError::Action(format!(
                "act '{node}' declares assign but is not a human decision act: it has no package"
            ))
        })?;
    if matches!(
        uses,
        crate::scheduler::shell::model::BLOCK_PACKAGE
            | crate::scheduler::shell::model::PARALLEL_PACKAGE
            | crate::scheduler::shell::model::SEQUENCE_PACKAGE
    ) {
        return Err(crate::ActError::Action(format!(
            "act '{node}' is container '{uses}'; declare assign on its human decision steps, not on the container"
        )));
    }
    if uses == crate::scheduler::shell::model::SUBFLOW_PACKAGE {
        return Err(crate::ActError::Action(format!(
            "act '{node}' uses '{uses}', which is not a human decision act (ActRunAs::Irq)"
        )));
    }
    // The import's auto decision is the package's run-bound: `Func` and `Msg`
    // self-complete, only `Irq` waits for a client decision.
    if auto_decision(snapshot, model_node.as_str())? {
        return Err(crate::ActError::Action(format!(
            "act '{node}' uses '{uses}', which is not a human decision act (ActRunAs::Irq)"
        )));
    }
    Ok(Some(assign.owner.clone()))
}

/// The `assign.owner` of a pushed act: refused on a container or the waiting
/// subflow, and on a package whose run-bound self-completes (`Func`/`Msg`).
/// The push commit wrote the node's decision into the run-bound maps, so the
/// same check the model's import made applies here.
fn pushed_declared_expression(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    task_id: &crate::scheduler::core::model::task_graph::TaskId,
) -> Result<Option<String>> {
    let Some(act) = crate::scheduler::shell::action::pushed_act_of(snapshot, task_id)? else {
        return Ok(None);
    };
    let Some(assign) = &act.assign else {
        return Ok(None);
    };
    let node = task_id.node.as_str();
    let uses = act.uses.trim();
    if matches!(
        uses,
        crate::scheduler::shell::model::BLOCK_PACKAGE
            | crate::scheduler::shell::model::PARALLEL_PACKAGE
            | crate::scheduler::shell::model::SEQUENCE_PACKAGE
    ) {
        return Err(crate::ActError::Action(format!(
            "act '{node}' is container '{uses}'; declare assign on its human decision steps, not on the container"
        )));
    }
    if uses == crate::scheduler::shell::model::SUBFLOW_PACKAGE {
        return Err(crate::ActError::Action(format!(
            "act '{node}' uses '{uses}', which is not a human decision act (ActRunAs::Irq)"
        )));
    }
    if auto_decision(snapshot, node)? {
        return Err(crate::ActError::Action(format!(
            "act '{node}' uses '{uses}', which is not a human decision act (ActRunAs::Irq)"
        )));
    }
    Ok(Some(assign.owner.clone()))
}

/// The act's resolved options with the assignment's routing fields merged in
/// (what the existing channel globs read). The declared options are preserved.
pub(super) fn options_with_assignment(
    options: serde_json::Value,
    assignment: &Assignment,
) -> serde_json::Value {
    let mut map = match options {
        serde_json::Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    map.insert(
        "owner".to_string(),
        serde_json::Value::String(assignment.owner.clone()),
    );
    map.insert(
        "assignee".to_string(),
        serde_json::Value::String(assignment.assignee.clone()),
    );
    serde_json::Value::Object(map)
}

/// Resolves an act's `assign.owner` expression at start, on the same headless
/// scope and through the same `fill_params` path a package's params use.
#[async_trait::async_trait]
pub(crate) trait AssignmentResolver: Send + Sync {
    /// The expression's value when it is a non-empty string; `None` for an
    /// empty or non-string resolution (the act must fail rather than run
    /// unowned).
    async fn resolve_owner(
        &self,
        scope: super::scope::ScopeSpec,
        expr: &str,
    ) -> Result<Option<String>>;
}

/// The serving runtime's assignment resolver: a headless task of the acting
/// node, exactly like the effect and predicate seams.
pub(crate) struct RuntimeAssignmentResolver {
    runtime: Arc<crate::scheduler::Runtime>,
}

impl RuntimeAssignmentResolver {
    pub(crate) fn new(runtime: Arc<crate::scheduler::Runtime>) -> Self {
        Self { runtime }
    }
}

#[async_trait::async_trait]
impl AssignmentResolver for RuntimeAssignmentResolver {
    async fn resolve_owner(
        &self,
        scope: super::scope::ScopeSpec,
        expr: &str,
    ) -> Result<Option<String>> {
        let (_, _, ctx) = super::scope::headless_scope(&self.runtime, &scope)?;
        // The same prepare the effect runner performs before it resolves the
        // act's params: seed the declared vars and seal the snapshot data the
        // expression may read.
        ctx.prepare().await?;
        let value = crate::utils::fill_params(&serde_json::Value::String(expr.to_string()), &ctx);
        Ok(match value {
            serde_json::Value::String(owner) if !owner.trim().is_empty() => Some(owner),
            _ => None,
        })
    }
}

impl KernelHost {
    /// Attach the assignment resolver: from then on an act's `assign.owner`
    /// resolves on the engine's own environment at start (see
    /// [`AssignmentResolver`]).
    pub(crate) fn attach_assignment_resolver(&self, resolver: Arc<dyn AssignmentResolver>) {
        *self.assignment.write() = Some(resolver);
    }

    /// Resolve one act's `assign.owner` expression at start through the same
    /// headless scope and `fill_params` path its params use. `None` when no
    /// resolver is attached or the expression resolves to an empty/non-string
    /// value; the caller fails the act rather than leave it unowned.
    pub(super) async fn resolve_assignment_owner(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        shared: Option<&Arc<crate::scheduler::core::commit::aggregate::ProcessSnapshot>>,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        expr: &str,
    ) -> Result<Option<String>> {
        let Some(model) = model_of(snapshot)? else {
            return Ok(None);
        };
        let Some(resolver) = self.assignment.read().clone() else {
            return Ok(None);
        };
        let scope = self.scope_spec(snapshot, shared, task_id, model)?;
        resolver.resolve_owner(scope, expr).await
    }
}

/// Merge one assignment into an already built act payload. The start
/// operation uses this for the `Created` record: the payload is built from
/// the snapshot before the same commit writes `__assignment`.
pub(super) fn inject_payload_assignment(payload: &mut serde_json::Value, assignment: &Assignment) {
    payload["owner"] = serde_json::Value::String(assignment.owner.clone());
    payload["assignee"] = serde_json::Value::String(assignment.assignee.clone());
    payload["delegator"] = serde_json::Value::Null;
    payload["delegatee"] = serde_json::Value::Null;
    payload["rev"] = serde_json::json!(assignment.rev);
    payload["ref_msg_id"] = serde_json::Value::Null;
    if let Some(inputs) = payload
        .get_mut("inputs")
        .and_then(|inputs| inputs.as_object_mut())
    {
        let options = inputs.remove("options").unwrap_or(serde_json::Value::Null);
        inputs.insert(
            "options".to_string(),
            options_with_assignment(options, assignment),
        );
    }
}
