//! The `ApplyAction` operation: replaying a durable decision, and running the act's
//! effect.
//!
//! Phase 2 is the work: a client decision is read back from the `__action`
//! fact the deciding call committed before its claim (so a decision whose
//! deciding call never applied it — a crash, an expired lease — is re-planned
//! exactly like that call would have), and an auto-completion operation runs the
//! step's package (the attached effect runner; a host without one
//! auto-completes). Phase 3 folds that outcome into the durable transition —
//! the completion, the container fan-out, the parked execution, the failing
//! act's error walk, or the rewind a `Cancel`/`Back` leaves behind.

use super::*;
use crate::scheduler::{
    core::{commit::aggregate::TaskSnapshot, model::task_graph::TaskId},
    shell::*,
};

impl KernelHost {
    /// Phase 3 of one completion: the durable plan a decided outcome implies.
    /// A plain execution completes and propagates; a container (a branched
    /// step, or a branch over its own steps) fans its children out instead and
    /// parks in `Waiting` until the join completes it. `action` is `None` for
    /// the auto-completion operation. Only a completion fans out — every other
    /// client decision (Submit / Skip / Cancel / Abort / Fail) is terminal for
    /// the container and its unborn children, exactly like the legacy engine,
    /// where a step that is no longer running never schedules its branches.
    ///
    /// `effect` is what phase 2 already ran: the auto-completion operation's package
    /// outcome (the legacy `ActRunAs::Func` run, whose outputs commit as scope
    /// variables with the same atomic completion), and `None` for a client
    /// decision, which runs nothing. A step that is already deciding a caught
    /// error never reaches here — phase 2 answers that with no work at all,
    /// which keeps the operation's own commit as its bare remainder.
    // Like `task_operation_plan`, the planner mirrors the aggregate's own facts
    // (operation, execution, claim) as separate parameters; bundling them
    // into a struct would only rename the same eight fields.
    #[allow(clippy::too_many_arguments)]
    async fn completion_plan(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        id: &OperationId,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        task: &crate::scheduler::core::commit::aggregate::TaskSnapshot,
        claim: &OperationCommit,
        action: Option<crate::scheduler::core::commit::command::ActionKind>,
        inputs: Vec<(String, String)>,
        effect: Option<&effects::EffectOutcome>,
    ) -> Result<TransitionPlan> {
        match action {
            None => {
                let propagate_id = OperationId {
                    process_id: id.process_id.clone(),
                    task: id.task.clone(),
                    sequence: id.sequence + 1,
                };
                // The act's own effect ran *before* its children are
                // dispatched (see phase 2): the legacy act completes (or
                // fails) first, and a `catches` handler answers the act's
                // failure — never a failure of the children it already
                // dispatched.
                let facts = act_facts(snapshot, task_id);
                // A container dispatches its children instead of completing —
                // with the act's own outcome folded into the same atomic
                // commit. A leaf completes and propagates.
                if let Some(command) = self
                    .fanout_command(snapshot, task_id, claim, &task.model_node, &inputs)
                    .await?
                {
                    let mut plan = self.container_fanout_plan(snapshot, &command)?;
                    if let Some(outcome) = effect {
                        self.fold_effect_outcome(task_id, outcome, &mut plan);
                        plan.validate().map_err(runtime)?;
                    }
                    return Ok(plan);
                }
                // A step whose timeout branches are still in flight may not
                // complete yet: the legacy `auto_complete` counts every child
                // task, the timeout branches included. The act's own completion
                // is recorded and the execution waits for them (the container
                // join finishes it when the last one settles).
                if self.waits_for_timeouts(snapshot, task_id, task)? {
                    let mut plan =
                        self.park_for_children_plan(snapshot, id, task_id, task, claim, &inputs)?;
                    if let Some(outcome) = effect {
                        self.fold_effect_outcome(task_id, outcome, &mut plan);
                        plan.validate().map_err(runtime)?;
                    }
                    return Ok(plan);
                }
                let mut plan = self.task_operation_plan(
                    snapshot,
                    id,
                    task_id,
                    Some(task),
                    claim,
                    "completed",
                    "task.completed",
                )?;
                if let Some(outcome) = effect {
                    self.fold_effect_outcome(task_id, outcome, &mut plan);
                }
                // The act's Completed message rides the completion record.
                if let Some(payload) = self.operation_message_payload(
                    snapshot,
                    task_id,
                    id.task.node.as_str(),
                    facts.as_ref(),
                    Some(&plan.variable_writes),
                    None,
                )? {
                    plan.outbox[0].payload = payload.to_string();
                }
                plan.state_changes.push(StateChange {
                    task: task_id.clone(),
                    expected_revision: task.revision,
                    from: task.state,
                    to: crate::scheduler::core::model::task_graph::TaskState::Completed,
                });
                if !snapshot.operations.contains_key(&propagate_id) {
                    plan.operations.push(OperationIntent {
                        id: propagate_id,
                        kind: OperationKind::PropagateTask,
                        ready_at_ms: None,
                    });
                }
                Ok(plan)
            }
            Some(kind) => {
                // A client completion of a container dispatches its children
                // instead of completing it (the legacy `Next` on a running
                // step) — the step declaring `catches` included: the completion
                // is its *act* completing, which then runs into its handlers.
                if matches!(kind, ActionKind::Complete)
                    && let Some(command) = self
                        .fanout_command(snapshot, task_id, claim, &task.model_node, &inputs)
                        .await?
                {
                    return self.container_fanout_plan(snapshot, &command);
                }
                // A completion of a step whose timeout branches are still in
                // flight is its *act* completing, not the step: the execution
                // waits for the branches it fired (see `park_for_children_plan`).
                if matches!(
                    kind,
                    crate::scheduler::core::commit::command::ActionKind::Complete
                ) && self.waits_for_timeouts(snapshot, task_id, task)?
                {
                    return self
                        .park_for_children_plan(snapshot, id, task_id, task, claim, &inputs);
                }
                // A client's error on a step declaring `catches` is the legacy
                // `Step::on_error`: the step stays running and dispatches its
                // handlers instead of being decided `Failed` here.
                if let crate::scheduler::core::commit::command::ActionKind::Fail { code, message } =
                    &kind
                    && let Some(plan) = self
                        .catch_fanout(
                            snapshot,
                            claim,
                            task_id,
                            task,
                            &Failure::new(code.clone(), message.clone()),
                            &inputs,
                        )
                        .await?
                {
                    return Ok(plan);
                }
                let rewinds = matches!(kind, ActionKind::Cancel | ActionKind::Back);
                let back = matches!(kind, ActionKind::Back);
                let command = ActionCommand {
                    process_id: id.process_id.clone(),
                    task: task_id.clone(),
                    action: kind,
                    inputs,
                    source_operation: claim.clone(),
                };
                let mut plan = ActionPlanner::plan(snapshot, &command).map_err(runtime)?;
                // A `Cancel`/`Back` rewinds: the decision closes the source
                // generation and the re-entry that follows is a durable
                // operation created in this same commit, so the rewind can be
                // neither lost to a crash nor confused with a state the
                // client never decided. The target it names is a durable fact
                // too (`Back` the client's `to`, `Cancel` this execution).
                if rewinds {
                    let target = if back {
                        command
                            .inputs
                            .iter()
                            .find(|(name, _)| name == crate::utils::consts::ACT_TO)
                            .map(|(_, value)| value.clone())
                            .ok_or_else(|| {
                                ActError::Runtime("cannot find 'to' value in options".to_string())
                            })?
                    } else {
                        task_id.node.as_str().to_owned()
                    };
                    plan.variable_writes.push(VariableWrite {
                        scope: task_id.clone(),
                        name: REENTER_TARGET_VAR.to_string(),
                        value: target,
                    });
                    let reenter_id = OperationId {
                        process_id: id.process_id.clone(),
                        task: task_id.clone(),
                        sequence: id.sequence + 2,
                    };
                    if !snapshot.operations.contains_key(&reenter_id) {
                        plan.operations.push(OperationIntent {
                            id: reenter_id,
                            kind: OperationKind::ReenterTask,
                            ready_at_ms: None,
                        });
                    }
                    plan.validate().map_err(runtime)?;
                }
                // A client action that decides a `uses` execution announces it
                // on the act's own message, exactly like the legacy act.
                self.attach_operation_message(snapshot, task_id, &mut plan)?;
                Ok(plan)
            }
        }
    }

    /// Whether an execution may not complete yet because a timeout branch it
    /// fired is still in flight. The legacy `auto_complete` counts every child
    /// task — the timeout branches included — so a step whose act is done waits
    /// for them; the kernel parks the execution in `Waiting` (a waiting act is
    /// already parked) and the container join finishes it.
    fn waits_for_timeouts(
        &self,
        snapshot: &ProcessSnapshot,
        task_id: &TaskId,
        task: &TaskSnapshot,
    ) -> Result<bool> {
        let branches = self.timeout_branches(snapshot, &task.model_node)?;
        Ok(!branches.is_empty()
            && crate::scheduler::core::plan::timeout::has_live_children(
                snapshot, task_id, &branches,
            ))
    }

    /// The container fan-out plan: [`Fanout`] plus the
    /// park mark the join reads.
    ///
    /// The mark belongs to this path, not to the planner: the container path is
    /// the one that parks an execution for the children it admitted, while a
    /// timeout dispatch admits a branch under an execution that is still
    /// waiting for its own act — marking that one would let the branch's
    /// settlement complete a step nobody answered (see
    /// [`crate::scheduler::PARK_CHILDREN`]).
    fn container_fanout_plan(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        command: &crate::scheduler::core::plan::container::ContainerFanoutCommand,
    ) -> Result<TransitionPlan> {
        let mut plan = Fanout::plan(snapshot, command).map_err(runtime)?;
        // A container without children completes in this very operation: it never
        // parks, so it carries no mark.
        if !command.children.is_empty() {
            mark_parked_for_children(&mut plan, &command.source);
        }
        // The container's own message, and only for a `uses` step: its act
        // completed here (the legacy act's task event fired before the step's
        // `auto_complete` saw the children). A package-less container is still
        // `Running` — the legacy step's emit rule skips that state — and its
        // task message belongs to the join that completes it.
        let source_node = snapshot
            .tasks
            .get(&command.source)
            .map(|task| task.model_node.clone());
        let source_has_package = match &source_node {
            Some(node) => self.step_uses(snapshot, node)?.is_some(),
            None => false,
        };
        if source_has_package
            && let Some(payload) = self.operation_message_payload(
                snapshot,
                &command.source,
                command.source.node.as_str(),
                act_facts(snapshot, &command.source).as_ref(),
                Some(&plan.variable_writes),
                None,
            )?
        {
            plan.outbox.push(
                crate::scheduler::core::model::transition_plan::OutboxIntent {
                    key: format!(
                        "completed:{}:{}:{}",
                        command.process_id,
                        command.source.node.as_str(),
                        command.source.generation
                    ),
                    topic: "task.completed".into(),
                    payload: payload.to_string(),
                },
            );
        }
        plan.validate().map_err(runtime)?;
        Ok(plan)
    }

    /// Record that `execution` parked in `Waiting` for its children. The
    /// planner's own fact wins over a client input of the same private name.
    fn park_for_children_plan(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        id: &OperationId,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        task: &crate::scheduler::core::commit::aggregate::TaskSnapshot,
        claim: &OperationCommit,
        inputs: &[(String, String)],
    ) -> Result<TransitionPlan> {
        let mut plan = self.task_operation_plan(
            snapshot,
            id,
            task_id,
            Some(task),
            claim,
            "completed",
            "task.completed",
        )?;
        for (name, value) in inputs {
            plan.variable_writes.push(VariableWrite {
                scope: task_id.clone(),
                name: name.clone(),
                value: value.clone(),
            });
        }
        // The act is answered, so the execution is no longer waiting for it:
        // it resumes `Running` while the branches it fired settle (the legacy
        // step's `next` after its act completed) — the state the join decides,
        // marked as parked.
        if task.state == crate::scheduler::core::model::task_graph::TaskState::Waiting {
            plan.state_changes.push(StateChange {
                task: task_id.clone(),
                expected_revision: task.revision,
                from: crate::scheduler::core::model::task_graph::TaskState::Waiting,
                to: crate::scheduler::core::model::task_graph::TaskState::Running,
            });
        }
        mark_parked_for_children(&mut plan, task_id);
        // The act did complete: its message rides the record, exactly like a
        // leaf completion's. A package-less execution announces nothing here —
        // it is still running, and only its terminal decision is a message.
        if self.step_uses(snapshot, &task.model_node)?.is_some()
            && let Some(payload) = self.operation_message_payload(
                snapshot,
                task_id,
                id.task.node.as_str(),
                act_facts(snapshot, task_id).as_ref(),
                Some(&plan.variable_writes),
                None,
            )?
        {
            plan.outbox[0].payload = payload.to_string();
        }
        plan.validate().map_err(runtime)?;
        Ok(plan)
    }

    /// Fold one package outcome into the operation's plan: the returned
    /// outputs become the acting scope's writes. They commit with the
    /// completion, so a crash before the commit re-runs the package against
    /// the same pre-state.
    fn fold_effect_outcome(
        &self,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        outcome: &effects::EffectOutcome,
        plan: &mut TransitionPlan,
    ) {
        if let Some(outputs) = &outcome.outputs {
            for (name, value) in effects::outputs_as_writes(outputs) {
                plan.variable_writes.push(VariableWrite {
                    scope: task_id.clone(),
                    name,
                    value,
                });
            }
        }
    }

    /// The plan of an execution failure: the execution decided `Failed` with
    /// the error variables, its message on its own record, and the propagation
    /// that carries the error walk upward (the legacy `emit_error` path). An
    /// execution that already left the running states keeps its state — the
    /// bare operation remainder completes the operation. A step declaring `catches`
    /// never reaches the `Failed` decision: [`Self::catch_fanout`] keeps it
    /// running and dispatches its handlers instead.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn failure_plan(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        id: &OperationId,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        task: &crate::scheduler::core::commit::aggregate::TaskSnapshot,
        claim: &OperationCommit,
        failure: &Failure,
    ) -> Result<TransitionPlan> {
        if let Some(plan) = self
            .catch_fanout(snapshot, claim, task_id, task, failure, &[])
            .await?
        {
            return Ok(plan);
        }
        let propagate_id = OperationId {
            process_id: id.process_id.clone(),
            task: id.task.clone(),
            sequence: id.sequence + 1,
        };
        let mut plan = self.task_operation_plan(
            snapshot,
            id,
            task_id,
            Some(task),
            claim,
            "failed",
            "task.failed",
        )?;
        if task.state.is_terminal() {
            return Ok(plan);
        }
        plan.state_changes.push(StateChange {
            task: task_id.clone(),
            expected_revision: task.revision,
            from: task.state,
            to: crate::scheduler::core::model::task_graph::TaskState::Failed,
        });
        for (name, value) in [
            ("error.code", failure.code.clone()),
            ("error.message", failure.message.clone()),
        ] {
            plan.variable_writes.push(VariableWrite {
                scope: task_id.clone(),
                name: name.to_string(),
                value,
            });
        }
        if !snapshot.operations.contains_key(&propagate_id) {
            plan.operations.push(OperationIntent {
                id: propagate_id,
                kind: OperationKind::PropagateTask,
                ready_at_ms: None,
            });
        }
        if let Some(payload) = self.operation_message_payload(
            snapshot,
            task_id,
            id.task.node.as_str(),
            act_facts(snapshot, task_id).as_ref(),
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
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        id: &OperationId,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        task: &crate::scheduler::core::commit::aggregate::TaskSnapshot,
        claim: &OperationCommit,
        error: &crate::ActError,
    ) -> Result<TransitionPlan> {
        self.failure_plan(snapshot, id, task_id, task, claim, &package_failure(error))
            .await
    }

    /// The plan of a caught failure: the legacy `Step::on_error`, which keeps
    /// a failed step `Running`, pins the error variables its CATCH predicates
    /// read (`$ecode()` answers the legacy `ecode`; the ERROR sign marks the
    /// step as deciding), and dispatches every `catches` child as a container
    /// child. The step's own join ([`crate::scheduler::core::plan::container::CatchJoin`])
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
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        claim: &OperationCommit,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        task: &crate::scheduler::core::commit::aggregate::TaskSnapshot,
        failure: &Failure,
        inputs: &[(String, String)],
    ) -> Result<Option<TransitionPlan>> {
        if !matches!(
            task.state,
            crate::scheduler::core::model::task_graph::TaskState::Running
                | crate::scheduler::core::model::task_graph::TaskState::Waiting
        ) {
            return Ok(None);
        }
        let Some(model) = model_of(snapshot)? else {
            return Ok(None);
        };
        let Some(step) = find_step_in_model(&model, task.model_node.as_str()) else {
            return Ok(None);
        };
        if step.catches.is_empty() || is_catch_decider(snapshot, task_id) {
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
                crate::scheduler::core::plan::container::SIGN_VAR.to_string(),
                crate::scheduler::core::plan::container::SIGN_CATCH.to_string(),
            ));
            let mut plan = crate::scheduler::core::plan::container::ContainerChildPlan::Entry;
            if let Some(expr) = &catch.r#if {
                // The handler's own selection (the legacy `Step::init`
                // guard), evaluated with the error the step is deciding.
                match self
                    .evaluate_branch_predicate(snapshot, task_id, expr, &overlay)
                    .await?
                {
                    PredicateValue::True => {}
                    PredicateValue::False => {
                        plan = crate::scheduler::core::plan::container::ContainerChildPlan::Skip;
                        // The handler's own selection did not match. Mark it
                        // like a guard skip so the catch join can tell it from
                        // a handler the client selected and then skipped
                        // (`catch_as_skip` is still a caught error).
                        vars.push((
                            crate::scheduler::core::plan::propagation::GUARD_SKIP_VAR.to_string(),
                            serde_json::json!(true).to_string(),
                        ));
                    }
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
            children.push(crate::scheduler::core::plan::container::ContainerChild {
                node: catch.id.clone(),
                model_node: node_id(&catch.id)?,
                plan,
                predecessors: vec![
                    crate::scheduler::core::plan::container::ContainerPredecessor::Source,
                ],
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
        let command = crate::scheduler::core::plan::container::ContainerFanoutCommand {
            process_id: snapshot.process_id.clone(),
            source: task_id.clone(),
            source_operation: claim.clone(),
            children,
            inputs,
            source_park: crate::scheduler::core::model::task_graph::TaskState::Running,
        };
        let mut plan = Fanout::plan(snapshot, &command).map_err(runtime)?;
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
                // The deciding flow: durable plan state the catch join reads —
                // it is what tells a failed step apart from a container parked
                // for its children, both of which are `Running`. A raw engine
                // fact (`__`-private, never part of a predicate's scope).
                crate::scheduler::core::plan::container::FLOW_VAR.to_string(),
                crate::scheduler::core::plan::container::FLOW_CATCHES.to_string(),
            ),
        ] {
            plan.variable_writes.push(VariableWrite {
                scope: task_id.clone(),
                name,
                value,
            });
        }
        plan.validate().map_err(runtime)?;
        // The failed act's own message: the legacy act emitted `Error` before
        // the step recovered, so the client sees the failure the handler is
        // answering. Sequence-qualified, so the step's own later decision (a
        // resumed error) carries a record of its own.
        if let Some(payload) = self.operation_message_payload(
            snapshot,
            task_id,
            task_id.node.as_str(),
            act_facts(snapshot, task_id).as_ref(),
            None,
            Some((&failure.code, &failure.message)),
        )? {
            plan.outbox.push(
                crate::scheduler::core::model::transition_plan::OutboxIntent {
                    key: format!(
                        "failed:{}:{}:{}:{}",
                        snapshot.process_id,
                        task_id.node.as_str(),
                        task_id.generation,
                        claim.id.sequence
                    ),
                    topic: "task.failed".into(),
                    payload: payload.to_string(),
                },
            );
            plan.validate().map_err(runtime)?;
        }
        Ok(Some(plan))
    }

    /// Announce one execution's decided state on the planner's own outbox
    /// record: the legacy engine emitted a message per state transition — the
    /// act's for a `uses` step, the task's for a package-less one — so a client
    /// action that decides it (complete/submit/skip/cancel/abort/fail) must
    /// speak the same message. The record's topic already names the state; its
    /// opaque lifecycle payload becomes the operation's payload.
    fn attach_operation_message(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        plan: &mut TransitionPlan,
    ) -> Result<()> {
        let Some(change) = plan
            .state_changes
            .iter()
            .find(|change| &change.task == task_id)
        else {
            return Ok(());
        };
        if !snapshot.tasks.contains_key(task_id) {
            return Ok(());
        }
        let Some(topic) = message_topic(change.to) else {
            return Ok(());
        };
        // A `uses` step announces its act; a package-less execution announces
        // the task itself (`operation_message_payload` picks the shape).
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
        let error = (change.to == crate::scheduler::core::model::task_graph::TaskState::Failed)
            .then(|| (read("error.code"), read("error.message")));
        let error = error
            .as_ref()
            .map(|(code, message)| (code.as_str(), message.as_str()));
        let facts = act_facts(snapshot, task_id);
        if let Some(payload) = self.operation_message_payload(
            snapshot,
            task_id,
            task_id.node.as_str(),
            facts.as_ref(),
            Some(&plan.variable_writes),
            error,
        )? {
            match plan.outbox.first_mut() {
                Some(record) => record.payload = payload.to_string(),
                None => plan.outbox.push(
                    crate::scheduler::core::model::transition_plan::OutboxIntent {
                        key: format!(
                            "message:{}:{}:{}",
                            plan.process_id,
                            task_id.node.as_str(),
                            task_id.generation
                        ),
                        topic: topic.to_string(),
                        payload: payload.to_string(),
                    },
                ),
            }
        }
        Ok(())
    }

    /// The dynamic children of a `uses`-param container, or `None` when the
    /// model node is not one (or has already fanned out for this generation).
    ///
    /// `acts.core.parallel{ in, acts }` and `acts.core.sequence{ in, acts }`
    /// instantiate their template step (`<step>#item`, an `acts.core.block`
    /// over the same acts) once per `in` item — exactly the legacy expansion
    /// (`ParallelPackage`/`SequencePackage` built one block act per item, with
    /// the item's index and value on its options). The count is a run-time
    /// fact, so the children share the template's model node and take
    /// successive generations of it; each carries its item on its own scope as
    /// engine-private facts (`FANOUT_INDEX_VAR`/`FANOUT_VALUE_VAR`) that
    /// `scope_vars` answers as `index`/`value`. They are deliberately not run
    /// data: a completing item does not fold them into its parent, and no row,
    /// run output or message `inputs` carries them (legacy kept the same two
    /// on the act's options).
    ///
    /// `parallel` admits its items together (`Entry`, all sourced by the
    /// container); `sequence` chains them (`Entry` then `Queued` behind the
    /// previous sibling), like the legacy `build_acts(&acts, true)`.
    fn param_fanout_children(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        model: &crate::model::Workflow,
        model_node: &crate::scheduler::core::model::model_graph::NodeId,
    ) -> Result<Option<Vec<ContainerChild>>> {
        let Some(step) = find_step_in_model(model, model_node.as_str()) else {
            return Ok(None);
        };
        let Some(uses) = step.uses.as_deref() else {
            return Ok(None);
        };
        let parallel = match uses {
            model::PARALLEL_PACKAGE => true,
            model::SEQUENCE_PACKAGE => false,
            _ => return Ok(None),
        };
        // The `in` array: the params the start operation resolved and pinned — the
        // same facts every later message of the act carries — falling back to
        // a literal declaration, which is all a host without an effect runner
        // ever has (its expression form is unresolvable there, and the
        // container then fans out no items).
        let pinned = act_facts(snapshot, task_id).map(|facts| facts.params);
        let items = pinned
            .as_ref()
            .and_then(|params| params.get("in"))
            .or_else(|| step.params.get("in"))
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        let template_id = format!("{}#item", step.id);
        let template_node = node_id(&template_id)?;
        // A container whose template already left `Created` has fanned out for
        // this generation; a completion now is a force-finish, not a second
        // fan-out.
        if snapshot.tasks.values().any(|task| {
            task.parent.as_ref() == Some(task_id)
                && task.model_node == template_node
                && task.state != crate::scheduler::core::model::task_graph::TaskState::Created
        }) {
            return Ok(None);
        }
        let template_vars: Vec<(String, String)> = find_step_in_model(model, &template_id)
            .map(|template| {
                template
                    .vars
                    .iter()
                    .map(|var| (var.name.clone(), var.value.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let auto = auto_decision(snapshot, &template_id)?;
        let children = items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                // Private (engine-fact) names: the binding is readable as
                // `index`/`value` inside the item's scope (see `scope_vars`),
                // but it is not run data — completing the template does not
                // fold it into the parent, and no row/output/message `inputs`
                // surface carries it. Legacy put the two on the act's options,
                // so neither did it.
                let mut vars = vec![
                    (
                        crate::utils::consts::FANOUT_INDEX_VAR.to_string(),
                        serde_json::json!(index).to_string(),
                    ),
                    (
                        crate::utils::consts::FANOUT_VALUE_VAR.to_string(),
                        item.to_string(),
                    ),
                ];
                vars.extend(template_vars.iter().cloned());
                ContainerChild {
                    node: template_id.clone(),
                    model_node: template_node.clone(),
                    plan: if parallel || index == 0 {
                        ContainerChildPlan::Entry
                    } else {
                        ContainerChildPlan::Queued
                    },
                    predecessors: if parallel || index == 0 {
                        vec![ContainerPredecessor::Source]
                    } else {
                        vec![ContainerPredecessor::Sibling(index - 1)]
                    },
                    vars,
                    auto,
                }
            })
            .collect();
        Ok(Some(children))
    }

    /// The fan-out command for a container execution, or `None` when the
    /// model node is a plain leaf. Branch selection goes through the kernel's
    /// own `Decide`; the inner chain of a branch is queued
    /// behind its first step.
    async fn fanout_command(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        claim: &OperationCommit,
        model_node: &crate::scheduler::core::model::model_graph::NodeId,
        inputs: &[(String, String)],
    ) -> Result<Option<ContainerFanoutCommand>> {
        let Some(model) = model_of(snapshot)? else {
            return Ok(None);
        };
        // A `uses`-param container (`acts.core.parallel`/`acts.core.sequence`)
        // instantiates its template step once per `in` item — the legacy
        // engine's per-item block task, which only the run can count. The
        // children are that one model node at successive generations, each
        // carrying its item as `index`/`value` on its own scope (the
        // template's acts read them through the scope chain).
        if let Some(children) = self.param_fanout_children(snapshot, task_id, &model, model_node)? {
            return Ok(Some(ContainerFanoutCommand {
                process_id: snapshot.process_id.clone(),
                source: task_id.clone(),
                source_operation: claim.clone(),
                children,
                inputs: inputs.to_vec(),
                // Like every container: the source parks `Running` while its
                // children run, and the join decides it once they settled.
                source_park: crate::scheduler::core::model::task_graph::TaskState::Running,
            }));
        }
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
            ContainerShape::Steps(steps) => steps.iter().map(|step| step.id.as_str()).collect(),
        };
        if snapshot.tasks.values().any(|task| {
            task.parent.as_ref() == Some(task_id)
                && declared.contains(&task.model_node.as_str())
                && task.state != crate::scheduler::core::model::task_graph::TaskState::Created
        }) {
            return Ok(None);
        }
        let children = match shape {
            ContainerShape::Branches(branches) => {
                let mut candidates = Vec::with_capacity(branches.len());
                for branch in branches {
                    candidates.push(BranchCandidate {
                        node: branch.id.clone(),
                        model_node: node_id(&branch.id)?,
                        predicate: match &branch.r#if {
                            Some(expr) => {
                                self.evaluate_branch_predicate(snapshot, task_id, expr, inputs)
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
                // the operation. Such a branch is never selected and never decides
                // anything for its siblings; in particular an `else` sibling
                // is blocked: the legacy else runs only when every `if`
                // sibling *skipped*, and an errored one did not.
                let branch_error = |node: &str| -> Option<String> {
                    candidates
                        .iter()
                        .find(|candidate| candidate.node == node)
                        .and_then(|candidate| match &candidate.predicate {
                            PredicateValue::Invalid { message } => Some(message.clone()),
                            _ => None,
                        })
                };
                let has_invalid = candidates
                    .iter()
                    .any(|candidate| branch_error(&candidate.node).is_some());
                let decidable: Vec<BranchCandidate> = candidates
                    .iter()
                    .filter(|candidate| {
                        branch_error(&candidate.node).is_none()
                            && !(has_invalid && candidate.is_else)
                    })
                    .cloned()
                    .collect();
                let selected: std::collections::HashSet<String> = if decidable.is_empty() {
                    Default::default()
                } else {
                    let command = BranchCommand {
                        process_id: snapshot.process_id.clone(),
                        branch: task_id.clone(),
                        candidates: decidable,
                        predicate_basis: PredicateBasis::from_snapshot(snapshot),
                        unresolved_dependencies: Vec::new(),
                        source_operation: claim.clone(),
                    };
                    match infallible(Decide::plan((), &command)) {
                        BranchDecision::Selected(selected) => selected
                            .into_iter()
                            .map(|candidate| candidate.node)
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
                        // its first operation reads the pinned failure and decides
                        // this execution `Failed`, so the parity floor keeps
                        // its terminal execution (the legacy branch task).
                        if let Some(message) = branch_error(&branch.id) {
                            vars.push((
                                BRANCH_ERROR_VAR.to_string(),
                                serde_json::json!({ "code": "", "message": message }).to_string(),
                            ));
                        }
                        Ok(ContainerChild {
                            node: branch.id.clone(),
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
                        node: step.id.clone(),
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
        };
        Ok(Some(ContainerFanoutCommand {
            process_id: snapshot.process_id.clone(),
            source: task_id.clone(),
            source_operation: claim.clone(),
            children,
            inputs: inputs.to_vec(),
            // A container that fanned out on its own completion stays
            // `Running` while its children run — the legacy step's state, which
            // is what `auto_complete` decides when the last child settled. A
            // source that was still `Waiting` (its act just answered) resumes
            // `Running` here, exactly like the legacy step's `next`.
            source_park: crate::scheduler::core::model::task_graph::TaskState::Running,
        }))
    }
}

/// Phase 2's product for this kind: what the act's completion was decided to
/// be.
pub(in crate::scheduler::shell) enum Effect {
    /// Nothing to commit: the execution is already deciding a caught error, the
    /// state moved past the decision while its intent was in flight, or the
    /// decision landed on an execution that can no longer take it. The operation's
    /// bare remainder completes the operation.
    Nothing,
    /// A durable client decision to replay.
    Decision {
        action: crate::scheduler::core::commit::command::ActionKind,
        inputs: Vec<(String, String)>,
    },
    /// The auto-completion operation: the act's own package ran, or its node carries
    /// no package at all.
    Completed(Option<effects::EffectOutcome>),
    /// The act's package failed: the error walk takes over.
    Failed(ActError),
}

/// The completion operation: the decision a client made, or the act's own work.
pub(super) struct ApplyAction;

#[async_trait::async_trait]
impl ActOperation for ApplyAction {
    /// Nothing to check before the claim: a completion always has its work.
    async fn precheck(&self, _host: &KernelHost, _pending: &Pending<'_>) -> Result<Precheck> {
        Ok(Precheck::Proceed)
    }

    async fn run(&self, host: &KernelHost, operation: &mut Operation<'_>) -> Result<Outcome> {
        let snapshot = operation.snapshot;
        let task_id = &operation.task_id;
        let task = operation
            .task
            .ok_or_else(|| ActError::Runtime(format!("execution {task_id:?} not found")))?;
        // A durable client decision is what this operation replays: the `__action`
        // fact is committed *before* the claim and the effect, so a decision
        // whose deciding call never applied it (a crash, an expired lease) is
        // re-planned exactly like that call would have. The execution's own
        // state is not the verdict — a `Cancel`'s target is terminal by
        // construction (`Completed|Submitted` -> `Backed`) — so reading the
        // state first would silently drop a cancel the client was already told
        // about.
        let fact = snapshot
            .variables
            .get(&(task_id.clone(), action_fact_var(operation.id.sequence)))
            .cloned();
        let effect = match fact {
            Some(fact) => {
                let (action, inputs) = decode_action_fact(&fact)?;
                // The step is already deciding a caught error (the sign its
                // failure operation pinned): no later operation may complete, fail or fan
                // it out — the catch join decides it. The client path refuses
                // such an action at the preflight; what reaches here is the
                // stale auto operation a failure raced, which keeps the operation's
                // own commit as its bare remainder.
                if is_catch_decider(snapshot, task_id)
                    || !crate::scheduler::core::commit::command::action_allowed(task.state, &action)
                {
                    // The state moved past the decision while the intent was in
                    // flight (an earlier decision won, a cascade closed the
                    // execution): the durable remainder is the empty plan, not
                    // an error.
                    Effect::Nothing
                } else {
                    Effect::Decision { action, inputs }
                }
            }
            None if !matches!(
                task.state,
                crate::scheduler::core::model::task_graph::TaskState::Running
                    | crate::scheduler::core::model::task_graph::TaskState::Waiting
            ) =>
            {
                // A decision landed while this operation was in flight (the cascade
                // skipped the execution, a cancel closed it): the durable
                // remainder is the empty plan, not an error.
                Effect::Nothing
            }
            None if is_catch_decider(snapshot, task_id) => Effect::Nothing,
            None => {
                // The act's own effect runs *before* its children are
                // dispatched: the legacy act completes (or fails) first, and a
                // `catches` handler answers the act's failure — never a failure
                // of the children it already dispatched.
                match host.step_effect(snapshot, task_id, &task.model_node)? {
                    Some((runner, request)) => {
                        // The effect's identity is the operation's durable
                        // identity (A1c): every retry, replan and recovery of
                        // this operation reuses this exact key.
                        let key =
                            crate::scheduler::core::model::effect::EffectKey::of(operation.id);
                        // A4: the effect runs outside the pid's command lane —
                        // a slow effect must not serialize the run's other
                        // commands. The lane is retaken the moment it returns;
                        // a command that committed in the window makes this
                        // operation's plan commit lose its guard, and that
                        // loss is the A1a `EffectUnknown` boundary — never a
                        // reason to run the effect again.
                        operation.lane.release();
                        // A8: the external effect has its own concurrency
                        // budget. The command lane is already free, so waiting
                        // here serializes nothing but effects.
                        let effect_lease = host
                            .enter_admission(
                                crate::scheduler::core::decision::AdmissionCategory::Effect,
                            )
                            .await;
                        let ran = runner.run(request, key).await;
                        drop(effect_lease);
                        operation.lane.resume().await;
                        match ran {
                            Ok(outcome) => Effect::Completed(Some(outcome)),
                            // A package error takes the failure path, where a
                            // step declaring `catches` keeps running and
                            // dispatches them (the legacy `Step::on_error`).
                            Err(error) => Effect::Failed(error),
                        }
                    }
                    None => Effect::Completed(None),
                }
            }
        };
        Ok(Outcome::Effect(effect))
    }

    async fn next(
        &self,
        host: &KernelHost,
        operation: &Operation<'_>,
        outcome: Outcome,
    ) -> Result<TransitionPlan> {
        let Outcome::Effect(effect) = outcome else {
            return Err(runtime(
                "the completion operation planned a foreign outcome",
            ));
        };
        let snapshot = operation.snapshot;
        let id = operation.id;
        let task_id = operation.task_id.clone();
        let claim = operation.claim;
        let task = operation
            .task
            .ok_or_else(|| ActError::Runtime(format!("execution {task_id:?} not found")))?;
        match effect {
            // The bare remainder of an operation with nothing to do: its own
            // commit, which completes the operation, is the whole effect.
            Effect::Nothing => host.task_operation_plan(
                snapshot,
                id,
                &task_id,
                Some(task),
                claim,
                "completed",
                "task.completed",
            ),
            Effect::Decision { action, inputs } => {
                host.completion_plan(
                    snapshot,
                    id,
                    &task_id,
                    task,
                    claim,
                    Some(action),
                    inputs,
                    None,
                )
                .await
            }
            // The auto-completion operation has no action: the act's own outcome is
            // what the plan folds in.
            Effect::Completed(effect) => {
                host.completion_plan(
                    snapshot,
                    id,
                    &task_id,
                    task,
                    claim,
                    None,
                    Vec::new(),
                    effect.as_ref(),
                )
                .await
            }
            Effect::Failed(error) => {
                if let Some(plan) = host
                    .catch_fanout(
                        snapshot,
                        claim,
                        &task_id,
                        task,
                        &package_failure(&error),
                        &[],
                    )
                    .await?
                {
                    return Ok(plan);
                }
                host.package_failure_plan(snapshot, id, &task_id, task, claim, &error)
                    .await
            }
        }
    }
}
