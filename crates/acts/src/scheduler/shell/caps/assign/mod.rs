//! The assignment capability: a human act's owner, its delegation, and the
//! per-task authorization rules.
//!
//! A human act's assignment is private durable state (`__assignment` /
//! `__delegation`) plus the rules that read it: who may decide, transfer,
//! delegate or undelegate, and how the state projects onto the client
//! payloads. The [`AssignmentResolver`] is the one replaceable seam — how a
//! declared `assign.owner` string becomes its owner at start. The
//! declaration lookup, the kernel's own resolver, and the snapshot-coupled
//! projections are the orchestration half of the same module; the pure
//! semantics live in [`owner`] and [`delegate`].
//!
//! The declaration (`Step::assign`) is resolved at act start through the same
//! headless scope and `fill_params` path the act's params use. The resolved
//! `__assignment` is a private task variable — the only authorization source —
//! and the message fields are its projection. An act that declares `assign`
//! without resolving to an owner fails at start: the engine never produces an
//! unowned approval.

use std::sync::Arc;

use super::Capability;

mod candidates;
mod delegate;
mod evaluate;
mod owner;
mod projection;
mod resolver;

pub(crate) use candidates::CANDIDATES_WAIT_VAR;
pub(crate) use delegate::{DELEGATION_VAR, Delegation};
pub(crate) use evaluate::{evaluate, names_business_data};
pub(crate) use owner::{ASSIGNMENT_VAR, Assignment};
pub(crate) use resolver::AssignmentResolver;

/// The assignment capability: the directory's identity and the namespace its
/// methods live under (see [`Capability`]). The pure semantics
/// (`impl Assign` in [`owner`]/[`delegate`]) and the orchestration
/// (`impl Assign` below, and in the shell's operation modules) are both this
/// type's methods — nothing of this capability is a free-floating function or
/// a `Workers` method.
#[derive(Debug, Clone, Copy, Default)]
pub struct Assign;

impl Capability for Assign {
    fn key(&self) -> &str {
        "kernel.assign"
    }
}

impl owner::Assignment {
    /// Read the assignment of one execution, when the act declares one.
    pub(crate) fn of(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        task: &crate::scheduler::core::model::task_graph::TaskId,
    ) -> crate::Result<Option<Self>> {
        let Some(raw) = crate::scheduler::shell::task_variable(snapshot, task, ASSIGNMENT_VAR)
        else {
            return Ok(None);
        };
        serde_json::from_str(raw)
            .map(Some)
            .map_err(crate::scheduler::shell::errors::runtime)
    }
}

impl delegate::Delegation {
    /// Read the active delegation of one execution. A missing variable and the
    /// persisted `null` both mean "no delegation"; a real record decodes.
    pub(crate) fn of(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        task: &crate::scheduler::core::model::task_graph::TaskId,
    ) -> crate::Result<Option<Self>> {
        let Some(raw) = crate::scheduler::shell::task_variable(snapshot, task, DELEGATION_VAR)
        else {
            return Ok(None);
        };
        serde_json::from_str::<Option<Self>>(raw).map_err(crate::scheduler::shell::errors::runtime)
    }
}

impl Assign {
    /// The caller-correctable checks a transfer must pass before any durable
    /// intent exists: the act is assigned and non-terminal, the caller owns it
    /// (or holds the capability), the target is named, and the client's
    /// `expect_rev` — when present — names the current revision. The plan
    /// repeats them for a replay; a refusal here consumes nothing.
    ///
    /// The target is a *business* user, not an access-control user: the engine
    /// implements no business user directory (a design non-goal), so `enabled`
    /// and the target's own decision permission are the calling system's
    /// checks. What the engine can prove it does: the target becomes the act's
    /// owner, and the act's per-task authorization then admits that owner.
    pub(crate) fn validate_transfer(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        state: crate::scheduler::core::model::task_graph::TaskState,
        inputs: &[(String, String)],
        subject: Option<&str>,
        can_assign: bool,
    ) -> crate::Result<()> {
        let node = task_id.node.as_str();
        let assignment = Assignment::of(snapshot, task_id)?.ok_or_else(|| {
            crate::ActError::Action(format!(
                "ENOTOWNER: act '{node}' has no assignment to transfer"
            ))
        })?;
        Self::authorize_assignment_change(
            Some(&assignment),
            subject,
            can_assign,
            "transfer",
            node,
        )?;
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
            .ok_or_else(|| {
                crate::ActError::Action("cannot find 'to' in act:transfer".to_string())
            })?;
        let _ = to;
        if let Some(raw) = input("expect_rev") {
            let expect = Self::parse_expect_rev(raw)?;
            if expect != assignment.rev {
                return Err(crate::ActError::Action(format!(
                    "EASSIGNMENT_CHANGED: act '{node}' assignment rev is {}, not {expect}",
                    assignment.rev
                )));
            }
        }
        Ok(())
    }

    /// The caller-correctable checks a delegation must pass before any durable
    /// intent exists: the act is assigned, the caller owns it (or holds the
    /// capability), the delegatee is named, an explicit `expires_at` parses,
    /// and the client's revision — when present — names the current one. The
    /// delegatee is a *business* user, never an access-control user: the
    /// engine checks that it is named and then admits it as the act's
    /// delegatee, and the calling system owns `enabled` and business-permission
    /// checks.
    pub(crate) fn validate_delegate(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        state: crate::scheduler::core::model::task_graph::TaskState,
        inputs: &[(String, String)],
        subject: Option<&str>,
        can_assign: bool,
        rev: Option<u64>,
    ) -> crate::Result<()> {
        let node = task_id.node.as_str();
        let assignment = Assignment::of(snapshot, task_id)?.ok_or_else(|| {
            crate::ActError::Action(format!(
                "ENOTOWNER: act '{node}' has no assignment to delegate"
            ))
        })?;
        Self::authorize_assignment_change(
            Some(&assignment),
            subject,
            can_assign,
            "delegate",
            node,
        )?;
        if state.is_terminal() {
            return Err(crate::ActError::Action(format!(
                "task('{node}') is not allowed to delegate"
            )));
        }
        let input = |name: &str| {
            inputs
                .iter()
                .rev()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        };
        input("delegatee")
            .map(str::trim)
            .filter(|delegatee| !delegatee.is_empty())
            .ok_or_else(|| {
                crate::ActError::Action("cannot find 'delegatee' in act:delegate".to_string())
            })?;
        if let Some(raw) = input("expires_at")
            .map(str::trim)
            .filter(|raw| !raw.is_empty())
        {
            Self::parse_expires_at(raw)?;
        }
        Self::compare_rev(&assignment, rev, node)
    }

    /// The caller-correctable checks an undelegation must pass: the act is
    /// assigned, the caller owns it (or holds the capability), an active
    /// delegation exists, the act is not terminal and the revision matches.
    pub(crate) fn validate_undelegate(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        state: crate::scheduler::core::model::task_graph::TaskState,
        subject: Option<&str>,
        can_assign: bool,
        rev: Option<u64>,
    ) -> crate::Result<()> {
        let node = task_id.node.as_str();
        let assignment = Assignment::of(snapshot, task_id)?.ok_or_else(|| {
            crate::ActError::Action(format!(
                "ENOTOWNER: act '{node}' has no assignment to undelegate"
            ))
        })?;
        Self::authorize_assignment_change(
            Some(&assignment),
            subject,
            can_assign,
            "undelegate",
            node,
        )?;
        if state.is_terminal() {
            return Err(crate::ActError::Action(format!(
                "task('{node}') is not allowed to undelegate"
            )));
        }
        Delegation::of(snapshot, task_id)?.ok_or_else(|| {
            crate::ActError::Action(format!(
                "ENODELEGATION: act '{node}' has no active delegation to revoke"
            ))
        })?;
        Self::compare_rev(&assignment, rev, node)
    }

    /// The durable id of one act's `Created` message, read from the record the
    /// start commit wrote. The id lives in the outbox payload, so it survives
    /// a restart and the transfer notification can reference the original
    /// message without inventing one; a record from an older kernel has none
    /// and yields `None`.
    pub(crate) fn created_message_id(
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

    /// The `assign.owner` string one act declares, after refusing the
    /// declaration where it cannot mean an owner: a package-less node, a
    /// `block`/`parallel`/`sequence` container, a `Func`/`Msg` leaf, or the
    /// waiting subflow. Only a human decision leaf (`ActRunAs::Irq`) may carry
    /// one. The refusal names the node.
    pub(crate) fn declared_owner(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        model_node: &crate::scheduler::core::model::model_graph::NodeId,
    ) -> crate::Result<Option<String>> {
        let Some(model) = crate::scheduler::shell::model_of(snapshot)? else {
            return Ok(None);
        };
        let Some(step) = crate::scheduler::shell::find_step_in_model(&model, model_node.as_str())
        else {
            // A pushed act is not in the immutable model: its declaration rides
            // the execution's durable `__act` fact.
            return Self::pushed_declared_owner(snapshot, task_id);
        };
        let Some(assign) = &step.assign else {
            return Ok(None);
        };
        let node = task_id.node.as_str();
        if assign.owner.is_none() && assign.candidates.is_none() {
            return Err(crate::ActError::Action(format!(
                "act '{node}' declares an empty assign: name an owner or a candidates pool"
            )));
        }
        // A candidates-only declaration carries no owner to resolve; its own
        // placement (containers welcome) is `declared_candidates`'s to judge.
        let Some(owner) = assign.owner.clone() else {
            return Ok(None);
        };
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
        // The import's auto decision is the package's run-bound: `Func` and
        // `Msg` self-complete, only `Irq` waits for a client decision.
        if crate::scheduler::shell::auto_decision(snapshot, model_node.as_str())? {
            return Err(crate::ActError::Action(format!(
                "act '{node}' uses '{uses}', which is not a human decision act (ActRunAs::Irq)"
            )));
        }
        // `Some` when the act declares an owner; `None` is a candidates-only
        // declaration (the pool the act computes at start, no single owner).
        Ok(Some(owner))
    }

    /// The `assign.owner` of a pushed act: refused on a container or the
    /// waiting subflow, and on a package whose run-bound self-completes
    /// (`Func`/`Msg`). The push commit wrote the node's decision into the
    /// run-bound maps, so the same check the model's import made applies here.
    fn pushed_declared_owner(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
    ) -> crate::Result<Option<String>> {
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
        if crate::scheduler::shell::auto_decision(snapshot, node)? {
            return Err(crate::ActError::Action(format!(
                "act '{node}' uses '{uses}', which is not a human decision act (ActRunAs::Irq)"
            )));
        }
        Ok(assign.owner.clone())
    }

    /// Project a decided act's assignment with the decision's attribution: an
    /// owner's decision records the owner as the assignee; a delegatee's
    /// records it as the assignee and the delegation's delegator/delegatee. An
    /// unattributed decision (ACL disabled, engine origin) keeps the
    /// assignment's own assignee. The subject is the durable fact the action
    /// recorded, never a client field.
    pub(crate) fn inject_decision_assignment(
        payload: &mut serde_json::Value,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        subject: Option<&str>,
    ) -> crate::Result<()> {
        let Some(assignment) = Assignment::of(snapshot, task_id)? else {
            return Ok(());
        };
        Self::inject_payload_assignment(payload, &assignment);
        let Some(subject) = subject else {
            return Ok(());
        };
        if subject == assignment.owner {
            // The owner decides for themselves, even while a delegation is
            // active: the decision is the owner's, not the delegatee's.
            Self::set_payload_assignee(payload, &assignment.owner);
        } else if let Some(delegation) = Delegation::of(snapshot, task_id)?
            && delegation.delegatee == subject
        {
            Self::inject_payload_delegation(payload, &delegation.delegator, subject);
        }
        Ok(())
    }
}

/// The serving runtime's assignment resolver: a headless task of the acting
/// node, exactly like the effect and predicate seams. The kernel's own
/// implementation of the owner-resolution seam — the slot a serving assembly
/// may replace (see [`crate::scheduler::shell::caps::CapServices`]).
pub(crate) struct RuntimeAssignmentResolver {
    runtime: Arc<crate::scheduler::Runtime>,
}

impl RuntimeAssignmentResolver {
    pub(crate) fn new(runtime: Arc<crate::scheduler::Runtime>) -> Self {
        Self { runtime }
    }
}

impl Capability for RuntimeAssignmentResolver {
    fn key(&self) -> &str {
        "kernel.assign"
    }
}

#[async_trait::async_trait]
impl AssignmentResolver for RuntimeAssignmentResolver {
    async fn resolve_owner(
        &self,
        scope: crate::scheduler::ScopeSpec,
        owner: &str,
    ) -> crate::Result<Option<String>> {
        let (_, _, ctx) = crate::scheduler::shell::scope::headless_scope(&self.runtime, &scope)?;
        // The same prepare the effect runner performs before it resolves the
        // act's params: seed the declared vars and seal the snapshot data the
        // expression may read.
        ctx.prepare().await?;
        // Owner resolution is local only: a literal user or a `${{ … }}`
        // expression on the engine's own environment.
        let participants = evaluate::evaluate_owner(&ctx, owner)?;
        match participants.as_slice() {
            [] => Ok(None),
            [owner] => Ok(Some(owner.id.clone())),
            participants => Err(crate::ActError::Action(format!(
                "assign.owner resolved to {} participants ({}); a human act has exactly one owner — declare assign.candidates and fan out with block/parallel/sequence for a multi-approver step",
                participants.len(),
                participants
                    .iter()
                    .map(|participant| participant.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }

    async fn compute_pool(
        &self,
        scope: crate::scheduler::ScopeSpec,
        expr: &crate::model::CandidateExpr,
    ) -> crate::Result<Vec<crate::model::Participant>> {
        let (_, _, ctx) = crate::scheduler::shell::scope::headless_scope(&self.runtime, &scope)?;
        // The same prepare the effect runner performs before it resolves the
        // act's params: seed the declared vars and seal the snapshot data the
        // expression may read.
        ctx.prepare().await?;
        evaluate::evaluate(&ctx, expr)
    }
}

impl crate::scheduler::shell::Workers {
    /// Attach the assignment resolver: from then on an act's `assign.owner`
    /// resolves on the engine's own environment at start (see
    /// [`AssignmentResolver`]). Slot plumbing, like every `attach_*` — the
    /// last attach wins; attach before driving operations.
    pub(crate) fn attach_assignment_resolver(&self, resolver: Arc<dyn AssignmentResolver>) {
        self.caps.write().assignment = Some(resolver);
    }

    /// Dispatch an act's `assign.owner` resolution to the attached resolver:
    /// read the slot, build the same headless scope its params use, and hand
    /// both over. `None` when no resolver is attached; the caller fails the
    /// act rather than leave it unowned.
    pub(crate) async fn resolve_assignment_owner(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        shared: Option<&Arc<crate::scheduler::core::commit::aggregate::ProcessSnapshot>>,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        owner: &str,
    ) -> crate::Result<Option<String>> {
        let Some(model) = crate::scheduler::shell::model_of(snapshot)? else {
            return Ok(None);
        };
        let Some(resolver) = self.caps.read().assignment.clone() else {
            return Ok(None);
        };
        let scope = self.scope_spec(snapshot, shared, task_id, model)?;
        resolver.resolve_owner(scope, owner).await
    }

    /// Dispatch an act's `assign.candidates` local computation to the
    /// attached resolver: read the slot, build the same headless scope the
    /// act's params use, and hand both over. `Err` — including when no
    /// resolver is attached — means "not locally computable": the caller asks
    /// the spec's `uses` package, whose answer is the definitive pool.
    pub(crate) async fn compute_candidates_pool(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        shared: Option<&Arc<crate::scheduler::core::commit::aggregate::ProcessSnapshot>>,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        expr: &crate::model::CandidateExpr,
    ) -> crate::Result<Vec<crate::model::Participant>> {
        let Some(model) = crate::scheduler::shell::model_of(snapshot)? else {
            return Err(crate::ActError::Action(
                "the run has no model to compute a candidates pool on".to_string(),
            ));
        };
        let Some(resolver) = self.caps.read().assignment.clone() else {
            return Err(crate::ActError::Action(
                "no assignment resolver is attached; the pool computes through the spec's uses package".to_string(),
            ));
        };
        let scope = self.scope_spec(snapshot, shared, task_id, model)?;
        resolver.compute_pool(scope, expr).await
    }
}
