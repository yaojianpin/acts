use super::*;

/// Whether an `ExecuteTask` hop may start this execution: a `Ready` one is the
/// ordinary scheduled start, and a `Created` one is a root the slot cap parked,
/// whose admission hop *is* its start hop (see
/// [`Admissions`](super::Admissions) and
/// `crate::scheduler::core::plan::concurrency`). A successor is `Created` only
/// until the propagation hop that schedules it commits its `Created → Ready`
/// move in the same plan as its `ExecuteTask` operation.
pub(super) fn startable(
    state: crate::scheduler::core::model::execution_graph::ExecutionState,
) -> bool {
    matches!(
        state,
        crate::scheduler::core::model::execution_graph::ExecutionState::Created
            | crate::scheduler::core::model::execution_graph::ExecutionState::Ready
    )
}

/// What one timeout dispatch round decided (see
/// [`KernelHost::timeout_round`]).
pub(super) enum TimeoutRound {
    /// The due branch and its decided guard: the hop dispatches it.
    Dispatch {
        branch: crate::scheduler::core::plan::timeout::TimeoutBranch,
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
pub(super) enum TimeoutGuard {
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
pub(super) fn next_plan_sequence(
    snapshot: &crate::scheduler::core::commit::aggregate::AggregateSnapshot,
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

/// Record one execution's decision flow in the hop's plan (`children` for a
/// container parking for the children it admitted, `catches` for a failed
/// execution that dispatched its handlers). The planner's own fact wins over a
/// client input of the same private name.
pub(super) fn mark_flow(
    plan: &mut TransitionPlan,
    execution: &crate::scheduler::core::model::execution_graph::ExecutionId,
    value: &str,
) {
    plan.variable_writes.retain(|write| {
        !(write.scope == *execution
            && write.name == crate::scheduler::core::plan::container::FLOW_VAR)
    });
    plan.variable_writes.push(VariableWrite {
        scope: execution.clone(),
        name: crate::scheduler::core::plan::container::FLOW_VAR.to_string(),
        value: value.to_string(),
    });
}

/// Record that one execution parked for the children it admitted.
pub(super) fn mark_parked_for_children(
    plan: &mut TransitionPlan,
    execution: &crate::scheduler::core::model::execution_graph::ExecutionId,
) {
    mark_flow(
        plan,
        execution,
        crate::scheduler::core::plan::container::FLOW_CHILDREN,
    );
}

/// Whether this execution is deciding a caught error: its failure hop pinned
/// the deciding flow ([`crate::scheduler::core::plan::container::FLOW_CATCHES`], the legacy
/// `Step::sign() == Sign::ERROR`), which the catch join answers and no later hop
/// may override.
pub(super) fn is_catch_decider(
    snapshot: &crate::scheduler::core::commit::aggregate::AggregateSnapshot,
    execution: &crate::scheduler::core::model::execution_graph::ExecutionId,
) -> bool {
    crate::scheduler::core::plan::container::deciding_catches(snapshot, execution)
}

/// The failure one package error reports: the kernel's own code and the
/// legacy `ecode` the failed act's data carried. The legacy engine wrote
/// `Error::from(ActError)` into the task data (`Task::set_err`), so only an
/// `Exception` carries a code and every other package error reads empty
/// through `$ecode()`.
pub(super) fn package_failure(error: &crate::ActError) -> Failure {
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
pub(super) fn branch_error(
    snapshot: &crate::scheduler::core::commit::aggregate::AggregateSnapshot,
    execution: &crate::scheduler::core::model::execution_graph::ExecutionId,
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

/// The model children of one container node, borrowed from the declaring
/// model. `Branches` is a branched step's selection, `Steps` a branch's own
/// chain — or the workflow root's seeded chain, which is why the root is a
/// container over `model.steps`. A step's `catches` are deliberately absent:
pub(super) enum ContainerShape<'a> {
    Branches(&'a [crate::model::Branch]),
    Steps(&'a [crate::model::Step]),
}

pub(super) fn walk_step<'a>(
    step: &'a crate::model::Step,
    target: &str,
) -> Option<ContainerShape<'a>> {
    if step.id == target {
        // Only the step's *branches* are a fan-out. Its `catches` are not: the
        // legacy tree builds them as `NodeOutputKind::Catch` children, which
        // `Node::children()` (Normal kind) does not return, so a successful
        // completion never runs into them — only `Step::on_error` dispatches
        // them, on the failure path ([`KernelHost::catch_fanout`]).
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

/// The step the flow falls through to when a step names no target of its own:
/// the sibling declared right after it inside the same container — the
/// workflow's top-level steps, a branch's steps, or a handler list. `None` for
/// the container's last child (the container itself then completes) and for a
/// node that is not a step (a branch, the root).
pub(super) fn declaration_successor(
    model: &crate::model::Workflow,
    target: &str,
) -> Option<String> {
    /// `Some(Some(next))` when `target` was found in this list, `Some(None)`
    /// when it was the last entry.
    fn in_list(steps: &[crate::model::Step], target: &str) -> Option<Option<String>> {
        for (index, step) in steps.iter().enumerate() {
            if step.id == target {
                return Some(steps.get(index + 1).map(|next| next.id.clone()));
            }
            for branch in model::step_branches(step).unwrap_or_default() {
                if let Some(found) = in_list(&branch.steps, target) {
                    return Some(found);
                }
            }
            if let Some(found) = in_list(&step.catches, target) {
                return Some(found);
            }
            if let Some(found) = in_list(&step.timeouts, target) {
                return Some(found);
            }
        }
        None
    }
    in_list(&model.steps, target).flatten()
}

/// Whether a step sits inside a `catches`/`timeouts` handler list. A handler
/// is dispatched by the failure machinery or the process tick, never by the
/// flow, so neither its guard nor its `next` is the flow's business: its `if`
/// selects it where it is dispatched (the catch fan-out, the tick), and its
/// completion reports to the container that dispatched it.
pub(super) fn is_handler_step(model: &crate::model::Workflow, target: &str) -> bool {
    fn in_handlers(steps: &[crate::model::Step], target: &str) -> bool {
        for step in steps {
            for branch in model::step_branches(step).unwrap_or_default() {
                if in_handlers(&branch.steps, target) {
                    return true;
                }
            }
            for handler in step.catches.iter().chain(step.timeouts.iter()) {
                if handler.id == target || in_handlers(std::slice::from_ref(handler), target) {
                    return true;
                }
            }
        }
        false
    }
    in_handlers(&model.steps, target)
}

/// Where the flow goes after a step settles, from the declaring model: the
/// step's own `next` when it declares one, otherwise the next declared sibling
/// ([`declaration_successor`]). `None` for a handler step ([`is_handler_step`]).
///
/// A step its own `if`/`while` guard skipped never takes its `next`: the
/// legacy guard's failure falls through to the chain, so a self or backward
/// `next` is not taken (that is what makes a guarded self-`next` loop
/// terminate).
pub(super) fn flow_target(
    model: &crate::model::Workflow,
    node: &str,
    guard_skipped: bool,
) -> Option<String> {
    if is_handler_step(model, node) {
        return None;
    }
    if guard_skipped {
        return declaration_successor(model, node);
    }
    let step = find_step_in_model(model, node)?;
    match step.next.as_ref().filter(|next| !next.trim().is_empty()) {
        Some(next) => Some(next.clone()),
        None => declaration_successor(model, node),
    }
}

pub(super) fn container_shape<'a>(
    model: &'a crate::model::Workflow,
    model_node: &crate::scheduler::core::model::model_graph::NodeId,
) -> Option<ContainerShape<'a>> {
    // The root's children are the seeded chain, and the root is a container
    // over them like any other parent: the legacy workflow task stays running
    // until every step it seeded settled (`auto_complete` counts them all), so
    // the kernel root parks and lets the join finish it. The chain itself
    // advances by propagation — the fan-out only admits the head.
    if model_node.as_str() == crate::utils::consts::TASK_ROOT_TID {
        return Some(ContainerShape::Steps(&model.steps));
    }
    model
        .steps
        .iter()
        .find_map(|step| walk_step(step, model_node.as_str()))
}

pub(super) fn node_id(name: &str) -> Result<crate::scheduler::core::model::model_graph::NodeId> {
    crate::scheduler::core::model::model_graph::NodeId::parse(name).map_err(runtime)
}

/// The step node with this id anywhere in the model (top level and inside
/// branches) — the effect seam's lookup for a `uses` step.
pub(super) fn find_step_in_model<'a>(
    model: &'a crate::model::Workflow,
    target: &str,
) -> Option<&'a crate::model::Step> {
    model.steps.iter().find_map(|step| find_step(step, target))
}

pub(super) fn find_step<'a>(
    step: &'a crate::model::Step,
    target: &str,
) -> Option<&'a crate::model::Step> {
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
