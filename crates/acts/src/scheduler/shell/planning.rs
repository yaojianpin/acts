use super::*;

/// Whether an `ExecuteTask` operation may start this execution: a `Ready` one is the
/// ordinary scheduled start, and a `Created` one is a root the slot cap parked,
/// whose admission operation *is* its start operation (see
/// [`Admissions`](super::Admissions) and
/// `crate::scheduler::core::plan::concurrency`). A successor is `Created` only
/// until the propagation operation that schedules it commits its `Created → Ready`
/// move in the same plan as its `ExecuteTask` operation.
pub(super) fn startable(state: crate::scheduler::core::model::task_graph::TaskState) -> bool {
    matches!(
        state,
        crate::scheduler::core::model::task_graph::TaskState::Created
            | crate::scheduler::core::model::task_graph::TaskState::Ready
    )
}

/// What one timeout dispatch round decided (see the `DispatchTimeout` operation's
/// phase 2).
pub(super) enum TimeoutRound {
    /// The due branch and its decided guard: the operation dispatches it.
    Dispatch {
        branch: crate::scheduler::core::plan::timeout::TimeoutBranch,
        guard: TimeoutGuard,
        cost_ms: i64,
        /// How many branches the node declares: the operation re-arms the timer while
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
    /// error): the branch is admitted and fails on its first operation, exactly like
    /// the legacy branch task whose `init` errored. The message is the pinned
    /// failure.
    Fail(String),
}

/// The next free operation sequence on one execution, counting both the
/// snapshot's records and the intents this plan already claims: an operation that adds
/// its own timer next to the act's completion op must not collide with it.
pub(super) fn next_plan_sequence(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    plan: &TransitionPlan,
    node: &str,
    generation: u64,
) -> u64 {
    let from_snapshot = snapshot.next_operation_sequence(node, generation);
    let from_plan = plan
        .operations
        .iter()
        .filter(|intent| {
            intent.id.task.node.as_str() == node && intent.id.task.generation == generation
        })
        .map(|intent| intent.id.sequence.saturating_add(1))
        .max()
        .unwrap_or(0);
    from_snapshot.max(from_plan)
}

/// Record one execution's decision flow in the operation's plan (`children` for a
/// container parking for the children it admitted, `catches` for a failed
/// execution that dispatched its handlers). The planner's own fact wins over a
/// client input of the same private name.
pub(super) fn mark_flow(
    plan: &mut TransitionPlan,
    task: &crate::scheduler::core::model::task_graph::TaskId,
    value: &str,
) {
    plan.variable_writes.retain(|write| {
        !(write.scope == *task && write.name == crate::scheduler::core::plan::container::FLOW_VAR)
    });
    plan.variable_writes.push(VariableWrite {
        scope: task.clone(),
        name: crate::scheduler::core::plan::container::FLOW_VAR.to_string(),
        value: value.to_string(),
    });
}

/// Record that one execution parked for the children it admitted.
pub(super) fn mark_parked_for_children(
    plan: &mut TransitionPlan,
    task: &crate::scheduler::core::model::task_graph::TaskId,
) {
    mark_flow(
        plan,
        task,
        crate::scheduler::core::plan::container::FLOW_CHILDREN,
    );
}

/// Whether this execution is deciding a caught error: its failure operation pinned
/// the deciding flow ([`crate::scheduler::core::plan::container::FLOW_CATCHES`], the legacy
/// `Step::sign() == Sign::ERROR`), which the catch join answers and no later operation
/// may override.
pub(super) fn is_catch_decider(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    task: &crate::scheduler::core::model::task_graph::TaskId,
) -> bool {
    crate::scheduler::core::plan::container::deciding_catches(snapshot, task)
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
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    task: &crate::scheduler::core::model::task_graph::TaskId,
) -> Option<Failure> {
    let raw = snapshot
        .variables
        .get(&(task.clone(), BRANCH_ERROR_VAR.to_string()))?;
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
        // legacy tree built them as catch-kind children, which the normal-child
        // walk did not return, so a successful completion never ran into them —
        // only `Step::on_error` dispatches
        // them, on the failure path (the completion operation's catch fan-out).
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Vars;
    use crate::model::Workflow;
    use crate::scheduler::core::model::task_graph::TaskState;

    /// Only the two scheduled states start an execution; every settled or
    /// waiting state does not.
    #[test]
    fn only_created_and_ready_are_startable() {
        assert!(startable(TaskState::Created));
        assert!(startable(TaskState::Ready));
        for state in [
            TaskState::Running,
            TaskState::Waiting,
            TaskState::Completed,
            TaskState::Submitted,
            TaskState::Backed,
            TaskState::Cancelled,
            TaskState::Failed,
            TaskState::Aborted,
            TaskState::Skipped,
            TaskState::Removed,
        ] {
            assert!(!startable(state), "{state:?}");
        }
    }

    /// A package failure carries the engine ecode only when the error is an
    /// engine exception; every other error reads empty through `$ecode()`.
    #[test]
    fn a_package_failure_carries_an_engine_ecode() {
        let exception = crate::ActError::Exception {
            ecode: "EBUSY".into(),
            message: "busy".into(),
        };
        let failure = package_failure(&exception);
        assert_eq!(failure.code, "EPACKAGE");
        assert_eq!(failure.ecode, "EBUSY");

        let plain = crate::ActError::Runtime("plain".into());
        assert_eq!(package_failure(&plain).ecode, "");
    }

    fn handler_model() -> Workflow {
        Workflow::new().with_step(|step| {
            step.with_id("s1")
                .with_uses("acts.transform.set", Vars::new())
                .with_branch(|branch| {
                    branch
                        .with_id("sb")
                        .with_if("true")
                        .with_step(|inner| inner.with_id("si"))
                })
                .with_timeout(|timeout| {
                    timeout.with_id("t1").with_branch(|branch| {
                        branch
                            .with_id("tb")
                            .with_if("true")
                            .with_step(|inner| inner.with_id("ti"))
                    })
                })
                .with_catch(|catch| {
                    catch.with_id("c1").with_branch(|branch| {
                        branch
                            .with_id("cb")
                            .with_if("true")
                            .with_step(|inner| inner.with_id("ci"))
                    })
                })
        })
    }

    /// A timeout branch is a container like any other: the fan-out looks its
    /// shape up through the handler walk.
    #[test]
    fn a_timeout_branch_container_is_walked() {
        let model = handler_model();
        let step = &model.steps[0];
        match walk_step(step, "tb") {
            Some(ContainerShape::Steps(steps)) => {
                assert_eq!(steps.len(), 1);
                assert_eq!(steps[0].id, "ti");
            }
            _ => panic!("the timeout's branch must be found through the handler walk"),
        }
        match walk_step(step, "t1") {
            Some(ContainerShape::Branches(branches)) => {
                assert_eq!(branches.len(), 1);
                assert_eq!(branches[0].id, "tb");
            }
            _ => panic!("the timeout handler owns its branch container"),
        }
        assert!(walk_step(step, "missing").is_none());
    }

    /// A handler list is a declaration chain of its own: the fall-through of
    /// one timeout step is the next timeout step.
    #[test]
    fn a_handler_list_has_its_own_declaration_order() {
        let model = Workflow::new().with_step(|step| {
            step.with_id("s1")
                .with_uses("acts.transform.set", Vars::new())
                .with_timeout(|timeout| timeout.with_id("t1"))
                .with_timeout(|timeout| timeout.with_id("t2"))
        });
        assert_eq!(declaration_successor(&model, "t1").as_deref(), Some("t2"));
        assert_eq!(declaration_successor(&model, "t2"), None);
        assert_eq!(declaration_successor(&model, "missing"), None);
    }

    /// A handler is found through a branch of the declaring step, and through
    /// a handler nested inside another handler.
    #[test]
    fn handlers_are_found_through_nested_containers() {
        let model = handler_model();
        assert!(is_handler_step(&model, "t1"));
        assert!(is_handler_step(&model, "c1"));
        // A handler's branch steps are the handler's own flow, not handlers.
        assert!(!is_handler_step(&model, "tb"));
        assert!(!is_handler_step(&model, "ti"));
        assert!(!is_handler_step(&model, "cb"));
        assert!(!is_handler_step(&model, "ci"));
        assert!(!is_handler_step(&model, "s1"));
        assert!(!is_handler_step(&model, "missing"));

        // A catch nested inside a branch of a plain step.
        let model = Workflow::new().with_step(|step| {
            step.with_id("s1").with_branch(|branch| {
                branch.with_id("b1").with_if("true").with_step(|inner| {
                    inner
                        .with_id("i1")
                        .with_catch(|catch| catch.with_id("deep"))
                })
            })
        });
        assert!(is_handler_step(&model, "deep"));
    }

    /// A handler step never flows: its own `next` is the dispatcher's, not the
    /// flow's.
    #[test]
    fn a_handler_step_has_no_flow_target() {
        let model = handler_model();
        assert_eq!(
            flow_target(&model, "t1", false),
            None,
            "a handler never flows"
        );
        assert_eq!(flow_target(&model, "ci", false), None);
        assert_eq!(
            flow_target(&model, "s1", false),
            None,
            "the last top-level step"
        );
        assert_eq!(flow_target(&model, "s1", true), None);
        assert_eq!(flow_target(&model, "missing", false), None);
    }

    /// A container shape is looked up by model node, the root included.
    #[test]
    fn container_shapes_resolve_by_model_node() {
        let model = handler_model();
        let root = crate::scheduler::core::model::model_graph::NodeId::parse(
            crate::utils::consts::TASK_ROOT_TID,
        )
        .unwrap();
        assert!(matches!(
            container_shape(&model, &root),
            Some(ContainerShape::Steps(_))
        ));
        let s1 = crate::scheduler::core::model::model_graph::NodeId::parse("s1").unwrap();
        assert!(matches!(
            container_shape(&model, &s1),
            Some(ContainerShape::Branches(_))
        ));
        let missing = crate::scheduler::core::model::model_graph::NodeId::parse("missing").unwrap();
        assert!(container_shape(&model, &missing).is_none());
    }
}
