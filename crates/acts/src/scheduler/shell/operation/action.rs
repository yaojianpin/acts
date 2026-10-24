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
                // A waiting act that ran an effect (the subflow) is not done
                // when the effect returns: the act waits for the external
                // answer the effect arranged (`ReturnToParent`). Its commit
                // just retires the effect operation; the act keeps its
                // waiting state and nothing propagates yet.
                let auto = snapshot
                    .variables
                    .get(&(task_id.clone(), "auto".to_string()))
                    .map(String::as_str)
                    != Some("0");
                if !auto && effect.is_some() {
                    return self.waiting_after_effect_plan(snapshot, id, task_id, task, claim);
                }
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
                // task, the timeout branches included. A pushed act
                // under the execution counts the same way. The act's own
                // completion is recorded and the execution waits for them
                // (the container join finishes it when the last one settles).
                if self.waits_for_timeouts(snapshot, task_id, task)?
                    || self.waits_for_pushed(snapshot, task_id)?
                {
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
                // A client `Push` decides nothing on the target: it
                // creates the pushed act under it. The target's own flow is
                // untouched — the pushed act announces itself when it starts.
                if matches!(
                    kind,
                    crate::scheduler::core::commit::command::ActionKind::Push
                ) {
                    return self.push_plan(snapshot, id, task_id, task, claim, &inputs);
                }
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
                // A live pushed act parks it the same way — the added
                // approver must act before the step completes.
                if matches!(
                    kind,
                    crate::scheduler::core::commit::command::ActionKind::Complete
                ) && (self.waits_for_timeouts(snapshot, task_id, task)?
                    || self.waits_for_pushed(snapshot, task_id)?)
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

    /// The plan of a waiting act whose effect just returned: the act keeps
    /// waiting for the answer the effect arranged (the subflow's child run),
    /// so only the effect operation retires. The execution resumes `Running`
    /// only if the start had to mark it so (a resolution failure deferred to
    /// the effect), because the waiting state is what tells the return
    /// operation the parent is ready for its decision.
    fn waiting_after_effect_plan(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        id: &OperationId,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        task: &TaskSnapshot,
        claim: &OperationCommit,
    ) -> Result<TransitionPlan> {
        let mut plan = self.task_operation_plan(
            snapshot,
            id,
            task_id,
            Some(task),
            claim,
            "waiting",
            "message.waiting",
        )?;
        if task.state == crate::scheduler::core::model::task_graph::TaskState::Running {
            plan.state_changes.push(StateChange {
                task: task_id.clone(),
                expected_revision: task.revision,
                from: crate::scheduler::core::model::task_graph::TaskState::Running,
                to: crate::scheduler::core::model::task_graph::TaskState::Waiting,
            });
        }
        plan.validate().map_err(runtime)?;
        Ok(plan)
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
            Some(node) => self.step_uses(snapshot, &command.source, node)?.is_some(),
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
        if self
            .step_uses(snapshot, task_id, &task.model_node)?
            .is_some()
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

    /// The plan of a client `Push`: a new act under the execution, from
    /// the act facts the client carried.
    ///
    /// The legacy `dispatch_act` grew a dynamic tree node under the acting
    /// task; the kernel's model is immutable for the life of a run, so the act
    /// rides the new execution instead — its facts live on the child scope
    /// ([`crate::scheduler::shell::action::PUSH_ACT_VAR`]) and every model
    /// lookup for the node falls back to them. The import-time auto/msg maps
    /// gain the node's entries in the same commit (`auto_decision`/
    /// `msg_decision` refuse a missing one), and the child is admitted
    /// `Ready` with its start operation, exactly like a fan-out child.
    ///
    /// The target itself is not decided: it stays `Waiting` (or keeps running
    /// its own act) and parks for the child when its own act is answered —
    /// the legacy `auto_complete` counted every child task — so the join
    /// finishes it once the pushed act settles.
    fn push_plan(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        id: &OperationId,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        task: &crate::scheduler::core::commit::aggregate::TaskSnapshot,
        claim: &OperationCommit,
        inputs: &[(String, String)],
    ) -> Result<TransitionPlan> {
        let act = crate::scheduler::shell::action::PushedAct::from_inputs(inputs)?;
        // The pushed node's id: `<target>#push<k>`, unique against every
        // execution the run ever held, so a replay against the same snapshot
        // mints the same id (idempotency) and two pushes never collide.
        let prefix = format!("{}#push", task_id.node.as_str());
        let nth = snapshot
            .tasks
            .keys()
            .filter(|existing| existing.node.as_str().starts_with(&prefix))
            .count()
            + 1;
        let node = format!("{prefix}{nth}");
        let child = TaskId::new(&node, 0).map_err(runtime)?;

        // The run-bound decisions the import built for declared nodes: the
        // pushed package's own `run_as`. An unregistered package is the `Irq`
        // default — the pushed act waits for its client.
        let run_as = self
            .effect
            .read()
            .as_ref()
            .and_then(|runner| runner.run_as(&act.uses));
        let auto = matches!(run_as, Some(crate::ActRunAs::Func | crate::ActRunAs::Msg));
        let msg = run_as == Some(crate::ActRunAs::Msg);

        let root = crate::scheduler::core::decision::root_task(snapshot)
            .ok_or_else(|| ActError::Runtime("the run has no root".to_string()))?;
        let mut writes = Vec::with_capacity(4);
        writes.push(VariableWrite {
            scope: child.clone(),
            name: crate::scheduler::shell::action::PUSH_ACT_VAR.to_string(),
            value: act.fact(),
        });
        writes.push(VariableWrite {
            scope: child.clone(),
            name: "auto".to_string(),
            value: if auto { "1" } else { "0" }.to_string(),
        });
        for (name, map_var) in [
            (auto, crate::scheduler::shell::model::AUTO_MAP_VAR),
            (msg, crate::scheduler::shell::model::MSG_MAP_VAR),
        ] {
            let mut map: serde_json::Map<String, serde_json::Value> =
                root_variable(snapshot, map_var)?
                    .map(|raw| serde_json::from_str(&raw))
                    .transpose()
                    .map_err(runtime)?
                    .unwrap_or_default();
            map.insert(node.clone(), serde_json::Value::Bool(name));
            writes.push(VariableWrite {
                scope: root.clone(),
                name: map_var.to_string(),
                value: serde_json::Value::Object(map).to_string(),
            });
        }

        let mut plan = self.task_operation_plan(
            snapshot,
            id,
            task_id,
            Some(task),
            claim,
            "pushed",
            "task.pushed",
        )?;
        plan.task_creates
            .push(crate::scheduler::core::model::transition_plan::TaskCreate {
                task: child.clone(),
                model_node: node_id(&node)?,
                parent: Some(task_id.clone()),
                predecessors: Vec::new(),
                initial_state: crate::scheduler::core::model::task_graph::TaskState::Ready,
            });
        plan.variable_writes = writes;
        plan.operations.push(OperationIntent {
            id: OperationId {
                process_id: id.process_id.clone(),
                task: child,
                sequence: 0,
            },
            kind: OperationKind::ExecuteTask,
            ready_at_ms: None,
        });
        plan.validate().map_err(runtime)?;
        Ok(plan)
    }

    /// Whether the execution holds a live child the model does not declare —
    /// a pushed act still running under it. The legacy `auto_complete`
    /// counted every child task, so the execution's own completion waits for
    /// it (the join finishes it once it settles); a declared child — a fanned
    /// branch, a timeout one — is not: a client completion of a fanned
    /// container stays the force-finish it always was.
    fn waits_for_pushed(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
    ) -> Result<bool> {
        let Some(model) = model_of(snapshot)? else {
            return Ok(false);
        };
        let live = snapshot.tasks.iter().any(|(child, task)| {
            task.parent.as_ref() == Some(task_id)
                && !task.state.is_terminal()
                && find_step_in_model(&model, child.node.as_str()).is_none()
        });
        Ok(live)
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
        // Borrow the snapshot field, not the whole operation: the effect phase
        // releases the command lane below, which is a second field.
        let snapshot = operation.snapshot.as_ref();
        let task_id = &operation.task_id;
        let task = operation
            .task()
            .cloned()
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
                match host.step_effect(
                    snapshot,
                    Some(&operation.snapshot),
                    task_id,
                    &task.model_node,
                )? {
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
                            Ok(outcome) => match outcome.decision {
                                // A decision the package deferred is the
                                // durable client decision it names: this
                                // operation plans and commits it exactly like
                                // one (`Effect::Decision`), in the same atomic
                                // transition as its own remainder — issuing it
                                // as a command would make it a concurrent
                                // writer this commit deterministically loses
                                // to (the `EffectUnknown` boundary below).
                                // Re-running the effect on a recovery re-derives
                                // the same decision, so the fold is crash-safe.
                                Some(deferred) => {
                                    let action = crate::Action::new(
                                        &snapshot.process_id,
                                        &task_id.wire(&snapshot.process_id),
                                        deferred.action,
                                        deferred.options,
                                    );
                                    match crate::scheduler::Runtime::kernel_kind(&action) {
                                        Ok(kind)
                                            if crate::scheduler::core::commit::command::action_allowed(
                                                task.state, &kind,
                                            ) =>
                                        {
                                            Effect::Decision {
                                                action: kind,
                                                inputs: crate::scheduler::Runtime::kernel_inputs(
                                                    &action.options,
                                                ),
                                            }
                                        }
                                        Ok(kind) => Effect::Failed(crate::ActError::Action(
                                            format!(
                                                "action {:?} is not allowed on execution '{}' in state {:?}",
                                                kind,
                                                task_id.node.as_str(),
                                                task.state
                                            ),
                                        )),
                                        Err(error) => Effect::Failed(error),
                                    }
                                }
                                None => Effect::Completed(Some(outcome)),
                            },
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
        let snapshot = operation.snapshot();
        let id = operation.id;
        let task_id = operation.task_id.clone();
        let claim = operation.claim;
        let task = operation
            .task()
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
#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::core::{
        commit::command::ActionKind,
        model::{
            operation::{ClaimToken, OperationRecord},
            task_graph::{TaskId, TaskState},
        },
    };
    use crate::scheduler::shell::effects::EffectOutcome;

    fn host() -> std::sync::Arc<KernelHost> {
        KernelHost::in_memory().0
    }

    fn task(node: &str) -> TaskId {
        TaskId::new(node, 0).unwrap()
    }

    fn id(node: &str, sequence: u64) -> OperationId {
        OperationId {
            process_id: "p".into(),
            task: task(node),
            sequence,
        }
    }

    fn commit(id: &OperationId) -> OperationCommit {
        OperationCommit {
            id: id.clone(),
            claim: ClaimToken {
                owner: "test".into(),
                fence: 1,
                lease_until_ms: i64::MAX,
                heartbeat_revision: 0,
            },
        }
    }

    fn seeded() -> ProcessSnapshot {
        ProcessSpec::linear("p", "root", &["a"])
            .seed_snapshot()
            .unwrap()
    }

    fn from_workflow(workflow: &crate::model::Workflow) -> ProcessSnapshot {
        ProcessSpec::from_workflow("p", workflow, &|_| None)
            .unwrap()
            .seed_snapshot()
            .unwrap()
    }

    fn empty_plan(snapshot: &ProcessSnapshot) -> TransitionPlan {
        TransitionPlan {
            process_id: snapshot.process_id.clone(),
            expected: snapshot.version.clone(),
            task_creates: Vec::new(),
            state_changes: Vec::new(),
            variable_writes: Vec::new(),
            source_operation: None,
            operations: Vec::new(),
            outbox: Vec::new(),
        }
    }

    fn snapstate(snapshot: &mut ProcessSnapshot, task: &TaskId, state: TaskState) {
        snapshot.tasks.get_mut(task).unwrap().state = state;
    }

    /// A `Back` without its `to` input names no rewind target.
    #[tokio::test]
    async fn back_without_a_target_is_an_error() {
        let host = host();
        let mut snapshot = seeded();
        let task_id = task("a");
        let id = id("a", 0);
        snapstate(&mut snapshot, &task_id, TaskState::Waiting);
        let claim = commit(&id);
        let snapshot_task = snapshot.tasks.get(&task_id).unwrap();
        let error = host
            .completion_plan(
                &snapshot,
                &id,
                &task_id,
                snapshot_task,
                &claim,
                Some(ActionKind::Back),
                Vec::new(),
                None,
            )
            .await
            .unwrap_err();
        assert!(
            format!("{error}").contains("cannot find 'to' value"),
            "{error}"
        );
    }

    /// A rewind whose re-entry is already durable does not plan it again.
    #[tokio::test]
    async fn a_rewind_does_not_duplicate_its_reentry() {
        let host = host();
        let mut snapshot = seeded();
        let task_id = task("a");
        let id = id("a", 0);
        snapstate(&mut snapshot, &task_id, TaskState::Waiting);
        let reenter = OperationId {
            sequence: id.sequence + 2,
            ..id.clone()
        };
        snapshot.operations.insert(
            reenter.clone(),
            OperationRecord::pending(reenter, OperationKind::ReenterTask),
        );
        let claim = commit(&id);
        let snapshot_task = snapshot.tasks.get(&task_id).unwrap();
        let plan = host
            .completion_plan(
                &snapshot,
                &id,
                &task_id,
                snapshot_task,
                &claim,
                Some(ActionKind::Back),
                vec![(crate::utils::consts::ACT_TO.to_string(), "s1".into())],
                None,
            )
            .await
            .unwrap();
        assert!(
            plan.variable_writes
                .iter()
                .any(|write| write.name == REENTER_TARGET_VAR)
        );
        assert!(
            plan.operations
                .iter()
                .all(|intent| intent.kind != OperationKind::ReenterTask)
        );
    }

    /// A container completion dispatches its children instead of completing
    /// the step (the client `Next` on a running step).
    #[tokio::test]
    async fn a_container_completion_fans_out() {
        let host = host();
        let workflow = crate::model::Workflow::new().with_step(|step| {
            step.with_id("s1")
                .with_branch(|branch| branch.with_id("b1").with_if("true"))
        });
        let mut snapshot = from_workflow(&workflow);
        let task_id = task("s1");
        snapstate(&mut snapshot, &task_id, TaskState::Running);
        let id = id("s1", 0);
        let claim = commit(&id);
        let snapshot_task = snapshot.tasks.get(&task_id).unwrap();
        let plan = host
            .completion_plan(
                &snapshot,
                &id,
                &task_id,
                snapshot_task,
                &claim,
                Some(ActionKind::Complete),
                Vec::new(),
                None,
            )
            .await
            .unwrap();
        assert!(
            plan.task_creates
                .iter()
                .any(|create| create.task.node.as_str() == "b1"),
            "the selected branch is admitted: {plan:?}"
        );
    }

    /// The auto completion of a container fans out the same way.
    #[tokio::test]
    async fn an_auto_container_completion_fans_out() {
        let host = host();
        let workflow = crate::model::Workflow::new().with_step(|step| {
            step.with_id("s1")
                .with_branch(|branch| branch.with_id("b1").with_if("true"))
        });
        let mut snapshot = from_workflow(&workflow);
        let task_id = task("s1");
        snapstate(&mut snapshot, &task_id, TaskState::Running);
        let id = id("s1", 0);
        let claim = commit(&id);
        let snapshot_task = snapshot.tasks.get(&task_id).unwrap();
        let plan = host
            .completion_plan(
                &snapshot,
                &id,
                &task_id,
                snapshot_task,
                &claim,
                None,
                Vec::new(),
                None,
            )
            .await
            .unwrap();
        assert!(
            plan.task_creates
                .iter()
                .any(|create| create.task.node.as_str() == "b1")
        );
    }

    /// A `uses` container announces its own completion at the fan-out, and the
    /// folded effect outcome rides the same plan.
    #[tokio::test]
    async fn a_package_container_folds_its_effect_and_message() {
        let host = host();
        let workflow = crate::model::Workflow::new().with_step(|step| {
            step.with_id("s1")
                .with_uses("pkg.test", crate::Vars::new())
                .with_branch(|branch| branch.with_id("b1").with_if("true"))
        });
        // A package container self-completes, so its package is a `Func` at
        // import time (`auto = "1"`).
        let mut snapshot =
            ProcessSpec::from_workflow("p", &workflow, &|_| Some(crate::ActRunAs::Func))
                .unwrap()
                .seed_snapshot()
                .unwrap();
        let task_id = task("s1");
        snapstate(&mut snapshot, &task_id, TaskState::Running);
        let id = id("s1", 0);
        let claim = commit(&id);
        let snapshot_task = snapshot.tasks.get(&task_id).unwrap();
        let outcome = EffectOutcome {
            outputs: Some(crate::Vars::new().with("answer", 42)),
            decision: None,
        };
        let plan = host
            .completion_plan(
                &snapshot,
                &id,
                &task_id,
                snapshot_task,
                &claim,
                None,
                Vec::new(),
                Some(&outcome),
            )
            .await
            .unwrap();
        assert!(
            plan.variable_writes
                .iter()
                .any(|write| write.scope == task_id && write.name == "answer")
        );
        assert!(
            plan.outbox
                .iter()
                .any(|record| record.topic == "task.completed")
        );
    }

    /// A waiting act whose effect ran (the subflow) keeps its `Waiting` state
    /// and does not propagate; the return operation is what decides it later.
    #[tokio::test]
    async fn action_waiting_act_with_an_effect_parks_without_completing() {
        let host = host();
        let workflow = crate::model::Workflow::new().with_step(|step| {
            step.with_id("s1").with_uses(
                crate::scheduler::shell::model::SUBFLOW_PACKAGE,
                crate::Vars::new(),
            )
        });
        let mut snapshot =
            ProcessSpec::from_workflow("p", &workflow, &|_| Some(crate::ActRunAs::Func))
                .unwrap()
                .seed_snapshot()
                .unwrap();
        let task_id = task("s1");
        snapstate(&mut snapshot, &task_id, TaskState::Waiting);
        let id = id("s1", 0);
        let claim = commit(&id);
        let snapshot_task = snapshot.tasks.get(&task_id).unwrap();
        let plan = host
            .completion_plan(
                &snapshot,
                &id,
                &task_id,
                snapshot_task,
                &claim,
                None,
                Vec::new(),
                Some(&EffectOutcome::empty()),
            )
            .await
            .unwrap();
        assert!(
            plan.state_changes.is_empty(),
            "the act stays waiting: {plan:?}"
        );
        assert!(
            plan.operations.is_empty(),
            "nothing propagates while the child is in flight: {plan:?}"
        );
        assert!(
            plan.outbox
                .iter()
                .any(|record| record.topic == "message.waiting"),
            "the commit is the bare effect remainder: {plan:?}"
        );
    }

    /// A step waiting for a timeout branch parks instead of completing.
    #[tokio::test]
    async fn a_step_waiting_for_its_branches_parks() {
        let host = host();
        let workflow = crate::model::Workflow::new().with_step(|step| {
            step.with_id("s1")
                .with_timeout(|timeout| timeout.with_id("t1"))
        });
        let mut snapshot = from_workflow(&workflow);
        let task_id = task("s1");
        snapstate(&mut snapshot, &task_id, TaskState::Running);
        let mut branch = snapshot.tasks[&task_id].clone();
        branch.parent = Some(task_id.clone());
        branch.model_node =
            crate::scheduler::core::model::model_graph::NodeId::parse("t1").unwrap();
        branch.state = TaskState::Running;
        snapshot.tasks.insert(task("t1"), branch);
        let id = id("s1", 0);
        let claim = commit(&id);
        let snapshot_task = snapshot.tasks.get(&task_id).unwrap();
        let outcome = EffectOutcome {
            outputs: None,
            decision: None,
        };
        let plan = host
            .completion_plan(
                &snapshot,
                &id,
                &task_id,
                snapshot_task,
                &claim,
                None,
                Vec::new(),
                Some(&outcome),
            )
            .await
            .unwrap();
        assert!(
            plan.variable_writes
                .iter()
                .any(|write| write.name == crate::scheduler::core::plan::container::FLOW_VAR)
        );
        assert!(plan.state_changes.is_empty(), "the execution keeps running");
    }

    /// A leaf's completion records the act's message and propagates.
    #[tokio::test]
    async fn a_leaf_completion_propagates() {
        let host = host();
        let mut snapshot = seeded();
        let task_id = task("a");
        snapstate(&mut snapshot, &task_id, TaskState::Running);
        let id = id("a", 0);
        let claim = commit(&id);
        let snapshot_task = snapshot.tasks.get(&task_id).unwrap();
        let plan = host
            .completion_plan(
                &snapshot,
                &id,
                &task_id,
                snapshot_task,
                &claim,
                None,
                Vec::new(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(plan.state_changes[0].to, TaskState::Completed);
        assert_eq!(plan.outbox[0].topic, "task.completed");
        assert_eq!(plan.operations[0].kind, OperationKind::PropagateTask);
    }

    /// A client `Fail` on a step without handlers takes the error walk.
    #[tokio::test]
    async fn a_client_failure_without_handlers_fails_the_step() {
        let host = host();
        let mut snapshot = seeded();
        let task_id = task("a");
        snapstate(&mut snapshot, &task_id, TaskState::Running);
        let id = id("a", 0);
        let claim = commit(&id);
        let snapshot_task = snapshot.tasks.get(&task_id).unwrap();
        let plan = host
            .completion_plan(
                &snapshot,
                &id,
                &task_id,
                snapshot_task,
                &claim,
                Some(ActionKind::Fail {
                    code: "E1".into(),
                    message: "boom".into(),
                }),
                Vec::new(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(plan.state_changes[0].to, TaskState::Failed);
        assert!(
            plan.variable_writes
                .iter()
                .any(|write| write.name == "error.code")
        );
    }

    /// The park plan carries the client inputs, the resume and the act message.
    #[tokio::test]
    async fn the_park_plan_carries_inputs_and_resumes() {
        let host = host();
        let workflow = crate::model::Workflow::new()
            .with_step(|step| step.with_id("s1").with_uses("pkg.test", crate::Vars::new()));
        let mut snapshot = from_workflow(&workflow);
        let task_id = task("s1");
        snapstate(&mut snapshot, &task_id, TaskState::Waiting);
        let id = id("s1", 0);
        let claim = commit(&id);
        let snapshot_task = snapshot.tasks.get(&task_id).unwrap();
        let plan = host
            .park_for_children_plan(
                &snapshot,
                &id,
                &task_id,
                snapshot_task,
                &claim,
                &[("answer".to_string(), "42".to_string())],
            )
            .unwrap();
        assert!(
            plan.variable_writes
                .iter()
                .any(|write| write.name == "answer" && write.value == "42")
        );
        assert_eq!(plan.state_changes[0].from, TaskState::Waiting);
        assert_eq!(plan.state_changes[0].to, TaskState::Running);
        assert!(
            plan.variable_writes
                .iter()
                .any(|write| write.name == crate::scheduler::core::plan::container::FLOW_VAR)
        );
        assert!(!plan.outbox[0].payload.is_empty());
    }

    /// A failure on a step declaring `catches` dispatches its handlers and
    /// keeps the step running; every guard verdict is pinned.
    #[tokio::test]
    async fn a_caught_failure_dispatches_every_handler() {
        let host = host();
        let workflow = crate::model::Workflow::new().with_step(|step| {
            step.with_id("s1")
                .with_catch(|catch| catch.with_id("c_none"))
                .with_catch(|catch| catch.with_id("c_true").with_if("true"))
                .with_catch(|catch| catch.with_id("c_false").with_if("false"))
                .with_catch(|catch| catch.with_id("c_null").with_if("null"))
                .with_catch(|catch| catch.with_id("c_bad").with_if("1 + 1"))
        });
        let mut snapshot = from_workflow(&workflow);
        let task_id = task("s1");
        snapstate(&mut snapshot, &task_id, TaskState::Running);
        let id = id("s1", 0);
        let claim = commit(&id);
        let snapshot_task = snapshot.tasks.get(&task_id).unwrap();
        let plan = host
            .failure_plan(
                &snapshot,
                &id,
                &task_id,
                snapshot_task,
                &claim,
                &Failure::new("E1", "boom"),
            )
            .await
            .unwrap();
        assert_eq!(plan.task_creates.len(), 5);
        assert!(
            plan.task_creates
                .iter()
                .any(|create| create.task.node.as_str() == "c_false")
        );
        // The skipped handler is marked as a guard skip; the undecidable ones
        // carry the pinned branch error.
        assert!(plan.variable_writes.iter().any(|write| {
            write.scope.node.as_str() == "c_false"
                && write.name == crate::scheduler::core::plan::propagation::GUARD_SKIP_VAR
        }));
        for branch in ["c_null", "c_bad"] {
            assert!(
                plan.variable_writes.iter().any(|write| {
                    write.scope.node.as_str() == branch && write.name == BRANCH_ERROR_VAR
                }),
                "{branch} has a pinned failure"
            );
        }
        assert!(
            plan.outbox
                .iter()
                .any(|record| record.topic == "task.failed")
        );
    }

    /// A step whose failure handlers are not applicable takes the failure
    /// plan: terminal executions keep their state, live ones fail.
    #[tokio::test]
    async fn a_failure_on_a_terminal_step_is_a_bare_remainder() {
        let host = host();
        let mut snapshot = seeded();
        let task_id = task("a");
        snapstate(&mut snapshot, &task_id, TaskState::Completed);
        let id = id("a", 0);
        let claim = commit(&id);
        let snapshot_task = snapshot.tasks.get(&task_id).unwrap();
        let plan = host
            .failure_plan(
                &snapshot,
                &id,
                &task_id,
                snapshot_task,
                &claim,
                &Failure::new("E1", "boom"),
            )
            .await
            .unwrap();
        assert!(plan.state_changes.is_empty());
        assert!(plan.operations.is_empty());
    }

    /// A live execution fails with the error facts, the propagation hand-off
    /// and the failure message; an already durable hand-off is not repeated.
    #[tokio::test]
    async fn a_live_failure_walks_the_error_up() {
        let host = host();
        let mut snapshot = seeded();
        let task_id = task("a");
        snapstate(&mut snapshot, &task_id, TaskState::Running);
        let id = id("a", 0);
        let claim = commit(&id);
        let snapshot_task = snapshot.tasks.get(&task_id).unwrap();
        let plan = host
            .failure_plan(
                &snapshot,
                &id,
                &task_id,
                snapshot_task,
                &claim,
                &Failure::new("E1", "boom"),
            )
            .await
            .unwrap();
        assert_eq!(plan.state_changes[0].to, TaskState::Failed);
        assert!(
            plan.variable_writes
                .iter()
                .any(|write| write.name == "error.message")
        );
        assert_eq!(plan.operations[0].kind, OperationKind::PropagateTask);
        assert_eq!(plan.outbox[0].topic, "task.failed");

        // The propagation is already durable: no second intent.
        let propagate = OperationId {
            sequence: id.sequence + 1,
            ..id.clone()
        };
        snapshot.operations.insert(
            propagate.clone(),
            OperationRecord::pending(propagate, OperationKind::PropagateTask),
        );
        let snapshot_task = snapshot.tasks.get(&task_id).unwrap();
        let plan = host
            .failure_plan(
                &snapshot,
                &id,
                &task_id,
                snapshot_task,
                &claim,
                &Failure::new("E1", "boom"),
            )
            .await
            .unwrap();
        assert!(plan.operations.is_empty());
    }

    /// A catch fan-out only exists for a live execution of a declaring node.
    #[tokio::test]
    async fn inapplicable_catch_fanouts_answer_none() {
        let host = host();
        let workflow = crate::model::Workflow::new()
            .with_step(|step| step.with_id("s1").with_catch(|catch| catch.with_id("c1")));
        let mut snapshot = from_workflow(&workflow);
        let task_id = task("s1");
        snapstate(&mut snapshot, &task_id, TaskState::Completed);
        let id = id("s1", 0);
        let claim = commit(&id);
        let failure = Failure::new("E", "boom");
        let snapshot_task = snapshot.tasks.get(&task_id).unwrap();
        assert!(
            host.catch_fanout(&snapshot, &claim, &task_id, snapshot_task, &failure, &[])
                .await
                .unwrap()
                .is_none(),
            "a settled execution has no handlers to dispatch"
        );

        snapstate(&mut snapshot, &task_id, TaskState::Running);
        let mut without_model = snapshot.clone();
        let root = without_model
            .tasks
            .iter()
            .find(|(_, task)| task.parent.is_none())
            .map(|(id, _)| id.clone())
            .expect("the run has a root");
        without_model
            .variables
            .remove(&(root, crate::scheduler::shell::model::MODEL_VAR.to_string()));
        let snapshot_task = without_model.tasks.get(&task_id).unwrap();
        assert!(
            host.catch_fanout(
                &without_model,
                &claim,
                &task_id,
                snapshot_task,
                &failure,
                &[]
            )
            .await
            .unwrap()
            .is_none()
        );

        // The model exists but does not declare this node.
        let mut renamed = snapshot.clone();
        renamed.tasks.get_mut(&task_id).unwrap().model_node =
            crate::scheduler::core::model::model_graph::NodeId::parse("ghost").unwrap();
        let snapshot_task = renamed.tasks.get(&task_id).unwrap();
        assert!(
            host.catch_fanout(&renamed, &claim, &task_id, snapshot_task, &failure, &[])
                .await
                .unwrap()
                .is_none()
        );
    }

    /// A step that already dispatches its handlers never dispatches them a
    /// second time.
    #[tokio::test]
    async fn a_deciding_catch_step_answers_none() {
        let host = host();
        let workflow = crate::model::Workflow::new()
            .with_step(|step| step.with_id("s1").with_catch(|catch| catch.with_id("c1")));
        let mut snapshot = from_workflow(&workflow);
        let task_id = task("s1");
        snapstate(&mut snapshot, &task_id, TaskState::Running);
        snapshot.variables.insert(
            (
                task_id.clone(),
                crate::scheduler::core::plan::container::FLOW_VAR.to_string(),
            ),
            crate::scheduler::core::plan::container::FLOW_CATCHES.to_string(),
        );
        let id = id("s1", 0);
        let claim = commit(&id);
        let snapshot_task = snapshot.tasks.get(&task_id).unwrap();
        assert!(
            host.catch_fanout(
                &snapshot,
                &claim,
                &task_id,
                snapshot_task,
                &Failure::new("E", "boom"),
                &[],
            )
            .await
            .unwrap()
            .is_none()
        );
    }

    /// The message attachment answers the three "nothing to say" cases and
    /// both record shapes.
    #[tokio::test]
    async fn an_operation_message_attaches_to_the_planned_record() {
        let host = host();
        let workflow = crate::model::Workflow::new().with_step(|step| step.with_id("a"));
        let mut snapshot = from_workflow(&workflow);
        let task_id = task("a");
        snapstate(&mut snapshot, &task_id, TaskState::Running);

        // No state change for the execution.
        let mut plan = empty_plan(&snapshot);
        host.attach_operation_message(&snapshot, &task_id, &mut plan)
            .unwrap();
        assert!(plan.outbox.is_empty());

        // A change to a state that emits no message.
        plan.state_changes.push(StateChange {
            task: task_id.clone(),
            expected_revision: 0,
            from: TaskState::Running,
            to: TaskState::Running,
        });
        host.attach_operation_message(&snapshot, &task_id, &mut plan)
            .unwrap();
        assert!(plan.outbox.is_empty());

        // The planner's own record receives the payload.
        plan.state_changes[0].to = TaskState::Completed;
        plan.outbox.push(
            crate::scheduler::core::model::transition_plan::OutboxIntent {
                key: "message:p:a:0".into(),
                topic: "task.completed".into(),
                payload: String::new(),
            },
        );
        host.attach_operation_message(&snapshot, &task_id, &mut plan)
            .unwrap();
        assert!(!plan.outbox[0].payload.is_empty());

        // A change of an execution the aggregate no longer holds.
        let mut gone = snapshot.clone();
        gone.tasks.remove(&task_id);
        let mut plan = empty_plan(&snapshot);
        plan.state_changes.push(StateChange {
            task: task_id.clone(),
            expected_revision: 0,
            from: TaskState::Running,
            to: TaskState::Completed,
        });
        host.attach_operation_message(&gone, &task_id, &mut plan)
            .unwrap();
        assert!(plan.outbox.is_empty());

        // Without a record, the message creates one from the decided state.
        let mut plan = empty_plan(&snapshot);
        plan.state_changes.push(StateChange {
            task: task_id.clone(),
            expected_revision: 0,
            from: TaskState::Running,
            to: TaskState::Failed,
        });
        plan.variable_writes.push(VariableWrite {
            scope: task_id.clone(),
            name: "error.code".into(),
            value: serde_json::json!("E1").to_string(),
        });
        host.attach_operation_message(&snapshot, &task_id, &mut plan)
            .unwrap();
        assert_eq!(plan.outbox[0].topic, "task.failed");
        assert!(plan.outbox[0].payload.contains("E1"));
    }

    /// A dynamic container whose template already ran does not fan out again.
    #[tokio::test]
    async fn a_param_container_that_already_fanned_out_answers_none() {
        let host = host();
        let workflow = crate::model::Workflow::new().with_step(|step| {
            step.with_id("s1").with_uses(
                crate::scheduler::shell::model::PARALLEL_PACKAGE,
                crate::Vars::new().with("in", serde_json::json!([1, 2])),
            )
        });
        let mut snapshot = from_workflow(&workflow);
        let task_id = task("s1");
        snapstate(&mut snapshot, &task_id, TaskState::Running);
        let model = crate::scheduler::shell::facts::model_of(&snapshot)
            .unwrap()
            .expect("the model is durable");
        let node = snapshot.tasks[&task_id].model_node.clone();
        snapshot.tasks.insert(
            TaskId::new("s1#item", 0).unwrap(),
            crate::scheduler::core::commit::aggregate::TaskSnapshot {
                model_node: crate::scheduler::core::model::model_graph::NodeId::parse("s1#item")
                    .unwrap(),
                parent: Some(task_id.clone()),
                predecessors: vec![task_id.clone()],
                state: TaskState::Running,
                revision: 0,
            },
        );
        assert!(
            host.param_fanout_children(&snapshot, &task_id, &model, &node)
                .unwrap()
                .is_none()
        );
    }

    /// A container that already admits its children answers no second
    /// fan-out.
    #[tokio::test]
    async fn a_container_that_already_fanned_out_answers_none() {
        let host = host();
        let workflow = crate::model::Workflow::new().with_step(|step| {
            step.with_id("s1")
                .with_branch(|branch| branch.with_id("b1").with_if("true"))
        });
        let mut snapshot = from_workflow(&workflow);
        let task_id = task("s1");
        snapstate(&mut snapshot, &task_id, TaskState::Running);
        snapshot.tasks.insert(
            TaskId::new("b1", 0).unwrap(),
            crate::scheduler::core::commit::aggregate::TaskSnapshot {
                model_node: crate::scheduler::core::model::model_graph::NodeId::parse("b1")
                    .unwrap(),
                parent: Some(task_id.clone()),
                predecessors: vec![task_id.clone()],
                state: TaskState::Running,
                revision: 0,
            },
        );
        let id = id("s1", 0);
        let claim = commit(&id);
        let node = snapshot.tasks[&task_id].model_node.clone();
        assert!(
            host.fanout_command(&snapshot, &task_id, &claim, &node, &[])
                .await
                .unwrap()
                .is_none()
        );
    }

    /// The completion operation needs its execution in both phases.
    #[tokio::test]
    async fn a_missing_execution_is_an_error_in_both_phases() {
        let host = host();
        let snapshot = seeded();
        let id = id("ghost", 0);
        let claim = commit(&id);
        let mut lane = CommandLane::detached(&host);
        let mut operation = Operation {
            id: &id,
            claim: &claim,
            task_id: id.task.clone(),
            task: snapshot.tasks.get(&id.task).cloned(),
            snapshot: std::sync::Arc::new(snapshot.clone()),
            now_ms: 0,
            lane: &mut lane,
        };
        let run_error = match ApplyAction.run(&host, &mut operation).await {
            Err(error) => error,
            Ok(_) => panic!("a missing execution must fail the start"),
        };
        assert!(format!("{run_error}").contains("not found"), "{run_error}");
        let next_error = ApplyAction
            .next(&host, &operation, Outcome::Effect(Effect::Completed(None)))
            .await
            .unwrap_err();
        assert!(
            format!("{next_error}").contains("not found"),
            "{next_error}"
        );
    }

    /// A stale auto completion on a step already deciding its caught error is
    /// the bare remainder.
    #[tokio::test]
    async fn a_stale_completion_on_a_catch_decider_does_nothing() {
        let host = host();
        let workflow = crate::model::Workflow::new()
            .with_step(|step| step.with_id("s1").with_catch(|catch| catch.with_id("c1")));
        let mut snapshot = from_workflow(&workflow);
        let task_id = task("s1");
        snapstate(&mut snapshot, &task_id, TaskState::Running);
        snapshot.variables.insert(
            (
                task_id.clone(),
                crate::scheduler::core::plan::container::FLOW_VAR.to_string(),
            ),
            crate::scheduler::core::plan::container::FLOW_CATCHES.to_string(),
        );
        let id = id("s1", 0);
        let claim = commit(&id);
        let mut lane = CommandLane::detached(&host);
        let mut operation = Operation {
            id: &id,
            claim: &claim,
            task_id: id.task.clone(),
            task: snapshot.tasks.get(&id.task).cloned(),
            snapshot: std::sync::Arc::new(snapshot.clone()),
            now_ms: 0,
            lane: &mut lane,
        };
        let outcome = ApplyAction.run(&host, &mut operation).await.unwrap();
        assert!(matches!(outcome, Outcome::Effect(Effect::Nothing)));
    }

    /// A decision whose execution already moved past it is the bare
    /// remainder, not an error.
    #[tokio::test]
    async fn a_decision_on_a_settled_execution_does_nothing() {
        let host = host();
        let mut snapshot = seeded();
        let task_id = task("a");
        snapstate(&mut snapshot, &task_id, TaskState::Completed);
        snapshot.variables.insert(
            (task_id.clone(), action_fact_var(0)),
            action_fact(&ActionKind::Complete, &[]),
        );
        let id = id("a", 0);
        let claim = commit(&id);
        let mut lane = CommandLane::detached(&host);
        let mut operation = Operation {
            id: &id,
            claim: &claim,
            task_id: id.task.clone(),
            task: snapshot.tasks.get(&id.task).cloned(),
            snapshot: std::sync::Arc::new(snapshot.clone()),
            now_ms: 0,
            lane: &mut lane,
        };
        let outcome = ApplyAction.run(&host, &mut operation).await.unwrap();
        assert!(matches!(outcome, Outcome::Effect(Effect::Nothing)));
    }

    /// The task-operation plan is the bare remainder of `Effect::Nothing`.
    #[tokio::test]
    async fn nothing_plans_the_bare_remainder() {
        let host = host();
        let snapshot = seeded();
        let id = id("a", 0);
        let claim = commit(&id);
        let mut lane = CommandLane::detached(&host);
        let operation = Operation {
            id: &id,
            claim: &claim,
            task_id: id.task.clone(),
            task: snapshot.tasks.get(&id.task).cloned(),
            snapshot: std::sync::Arc::new(snapshot.clone()),
            now_ms: 0,
            lane: &mut lane,
        };
        let plan = ApplyAction
            .next(&host, &operation, Outcome::Effect(Effect::Nothing))
            .await
            .unwrap();
        assert_eq!(plan.source_operation.as_ref(), Some(&claim));
        assert!(plan.state_changes.is_empty());
    }

    /// A foreign outcome never reaches the completion planner.
    #[tokio::test]
    async fn a_foreign_outcome_is_an_error() {
        let host = host();
        let snapshot = seeded();
        let id = id("a", 0);
        let claim = commit(&id);
        let mut lane = CommandLane::detached(&host);
        let operation = Operation {
            id: &id,
            claim: &claim,
            task_id: id.task.clone(),
            task: snapshot.tasks.get(&id.task).cloned(),
            snapshot: std::sync::Arc::new(snapshot.clone()),
            now_ms: 0,
            lane: &mut lane,
        };
        assert!(
            ApplyAction
                .next(&host, &operation, Outcome::Planned)
                .await
                .is_err()
        );
    }

    /// A decision fact that reaches a step already deciding its caught error is
    /// dropped before the action is even classified: the catch join decides it.
    #[tokio::test]
    async fn a_decision_fact_on_a_catch_decider_does_nothing() {
        let host = host();
        let workflow = crate::model::Workflow::new()
            .with_step(|step| step.with_id("s1").with_catch(|catch| catch.with_id("c1")));
        let mut snapshot = from_workflow(&workflow);
        let task_id = task("s1");
        snapstate(&mut snapshot, &task_id, TaskState::Running);
        snapshot.variables.insert(
            (
                task_id.clone(),
                crate::scheduler::core::plan::container::FLOW_VAR.to_string(),
            ),
            crate::scheduler::core::plan::container::FLOW_CATCHES.to_string(),
        );
        snapshot.variables.insert(
            (task_id.clone(), action_fact_var(0)),
            action_fact(&ActionKind::Complete, &[]),
        );
        let id = id("s1", 0);
        let claim = commit(&id);
        let mut lane = CommandLane::detached(&host);
        let mut operation = Operation {
            id: &id,
            claim: &claim,
            task_id: id.task.clone(),
            task: snapshot.tasks.get(&id.task).cloned(),
            snapshot: std::sync::Arc::new(snapshot.clone()),
            now_ms: 0,
            lane: &mut lane,
        };
        let outcome = ApplyAction.run(&host, &mut operation).await.unwrap();
        assert!(matches!(outcome, Outcome::Effect(Effect::Nothing)));
    }

    /// A failed effect on a step without handlers takes the failure plan
    /// (there is no catch fan-out to answer it).
    #[tokio::test]
    async fn a_failed_effect_without_handlers_takes_the_failure_plan() {
        let host = host();
        let mut snapshot = seeded();
        let task_id = task("a");
        snapstate(&mut snapshot, &task_id, TaskState::Running);
        let id = id("a", 0);
        let claim = commit(&id);
        let mut lane = CommandLane::detached(&host);
        let operation = Operation {
            id: &id,
            claim: &claim,
            task_id: id.task.clone(),
            task: snapshot.tasks.get(&id.task).cloned(),
            snapshot: std::sync::Arc::new(snapshot.clone()),
            now_ms: 0,
            lane: &mut lane,
        };
        let plan = ApplyAction
            .next(
                &host,
                &operation,
                Outcome::Effect(Effect::Failed(crate::ActError::Package("boom".into()))),
            )
            .await
            .unwrap();
        assert_eq!(plan.state_changes[0].to, TaskState::Failed);
        assert_eq!(plan.outbox[0].topic, "task.failed");
    }

    /// A step waiting for its branches parks even when the effect outcome is
    /// absent (a client decision on an already effect-free act).
    #[tokio::test]
    async fn a_waiting_step_without_an_effect_outcome_still_parks() {
        let host = host();
        let workflow = crate::model::Workflow::new().with_step(|step| {
            step.with_id("s1")
                .with_timeout(|timeout| timeout.with_id("t1"))
        });
        let mut snapshot = from_workflow(&workflow);
        let task_id = task("s1");
        snapstate(&mut snapshot, &task_id, TaskState::Running);
        let mut branch = snapshot.tasks[&task_id].clone();
        branch.parent = Some(task_id.clone());
        branch.model_node =
            crate::scheduler::core::model::model_graph::NodeId::parse("t1").unwrap();
        branch.state = TaskState::Running;
        snapshot.tasks.insert(task("t1"), branch);
        let id = id("s1", 0);
        let claim = commit(&id);
        let snapshot_task = snapshot.tasks.get(&task_id).unwrap();
        let plan = host
            .completion_plan(
                &snapshot,
                &id,
                &task_id,
                snapshot_task,
                &claim,
                None,
                Vec::new(),
                None,
            )
            .await
            .unwrap();
        assert!(plan.state_changes.is_empty());
        assert!(
            plan.variable_writes
                .iter()
                .any(|write| write.name == crate::scheduler::core::plan::container::FLOW_VAR)
        );
    }

    /// A park announces the answered *act* and nothing else: a package-less
    /// execution — a branch node, a plain step — is still running, so its
    /// record carries no payload and its terminal state is announced by the
    /// join that completes it.
    #[tokio::test]
    async fn a_park_named_by_a_branch_node_has_no_message_payload() {
        let host = host();
        let workflow = crate::model::Workflow::new().with_step(|step| {
            step.with_id("s1")
                .with_uses("pkg.test", crate::Vars::new())
                .with_branch(|branch| branch.with_id("b1").with_if("true"))
        });
        let mut snapshot = from_workflow(&workflow);
        let task_id = task("b1");
        snapshot.tasks.insert(
            task_id.clone(),
            crate::scheduler::core::commit::aggregate::TaskSnapshot {
                model_node: crate::scheduler::core::model::model_graph::NodeId::parse("b1")
                    .unwrap(),
                parent: Some(task("s1")),
                predecessors: vec![task("s1")],
                state: TaskState::Waiting,
                revision: 0,
            },
        );
        let operation_id = id("b1", 0);
        let claim = commit(&operation_id);
        let snapshot_task = snapshot.tasks.get(&task_id).unwrap();
        let plan = host
            .park_for_children_plan(
                &snapshot,
                &operation_id,
                &task_id,
                snapshot_task,
                &claim,
                &[],
            )
            .unwrap();
        assert_eq!(plan.outbox.len(), 1);
        assert!(
            plan.outbox[0].payload.is_empty(),
            "a package-less execution announces nothing while it parks"
        );
        assert!(
            plan.variable_writes
                .iter()
                .any(|write| write.name == crate::scheduler::core::plan::container::FLOW_VAR)
        );
    }

    /// A dynamic container whose only template child is still `Created` has
    /// not fanned out for this generation yet.
    #[tokio::test]
    async fn a_param_container_with_only_created_children_fans_out() {
        let host = host();
        let workflow = crate::model::Workflow::new().with_step(|step| {
            step.with_id("s1").with_uses(
                crate::scheduler::shell::model::PARALLEL_PACKAGE,
                crate::Vars::new().with("in", serde_json::json!([1, 2])),
            )
        });
        let mut snapshot = from_workflow(&workflow);
        let task_id = task("s1");
        snapstate(&mut snapshot, &task_id, TaskState::Running);
        let model = crate::scheduler::shell::facts::model_of(&snapshot)
            .unwrap()
            .expect("the model is durable");
        let node = snapshot.tasks[&task_id].model_node.clone();
        let root = snapshot
            .tasks
            .iter()
            .find(|(_, task)| task.parent.is_none())
            .map(|(id, _)| id.clone())
            .expect("the run has a root");
        snapshot.variables.insert(
            (
                root,
                crate::scheduler::shell::model::AUTO_MAP_VAR.to_string(),
            ),
            serde_json::json!({ "s1#item": true }).to_string(),
        );
        snapshot.tasks.insert(
            TaskId::new("s1#item", 0).unwrap(),
            crate::scheduler::core::commit::aggregate::TaskSnapshot {
                model_node: crate::scheduler::core::model::model_graph::NodeId::parse("s1#item")
                    .unwrap(),
                parent: Some(task_id.clone()),
                predecessors: vec![task_id.clone()],
                state: TaskState::Created,
                revision: 0,
            },
        );
        let children = host
            .param_fanout_children(&snapshot, &task_id, &model, &node)
            .unwrap()
            .expect("a container with only created items fans out");
        assert_eq!(children.len(), 2);
    }

    /// A container fan-out announces only a `uses` source's act: a
    /// package-less source — a branch node here — is still running while its
    /// children settle, so the fan-out emits no `task.completed` for it (the
    /// join that completes it announces its terminal state).
    #[tokio::test]
    async fn a_container_plan_announces_no_package_less_source_message() {
        use crate::scheduler::core::plan::container::ContainerFanoutCommand;

        let host = host();
        let workflow = crate::model::Workflow::new().with_step(|step| {
            step.with_id("s1")
                .with_uses("pkg.test", crate::Vars::new())
                .with_branch(|branch| branch.with_id("b1").with_if("true"))
        });
        let mut snapshot = from_workflow(&workflow);
        let task_id = task("s1");
        snapstate(&mut snapshot, &task_id, TaskState::Running);
        let source = task("b1");
        snapshot.tasks.insert(
            source.clone(),
            crate::scheduler::core::commit::aggregate::TaskSnapshot {
                // The branch's own node, exactly as the fan-out admits it.
                model_node: crate::scheduler::core::model::model_graph::NodeId::parse("b1")
                    .unwrap(),
                parent: Some(task_id.clone()),
                predecessors: vec![task_id.clone()],
                state: TaskState::Running,
                revision: 0,
            },
        );
        let id = id("b1", 0);
        let claim = commit(&id);
        let command = ContainerFanoutCommand {
            process_id: snapshot.process_id.clone(),
            source,
            source_operation: claim,
            children: Vec::new(),
            inputs: Vec::new(),
            source_park: TaskState::Running,
        };
        let plan = host.container_fanout_plan(&snapshot, &command).unwrap();
        assert!(
            plan.outbox
                .iter()
                .all(|record| record.topic != "task.completed"),
            "a package-less source emits no act message: {:?}",
            plan.outbox
        );
    }
}
