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
        return Ok(None);
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
