//! The `PropagateTask` hop: carrying a settled execution into its successors.
//!
//! The hop runs nothing (phase 2 is the snapshot itself); phase 3 re-plans
//! propagation from the authoritative snapshot — the successors'
//! `Created → Ready` plus their `ExecuteTask` operations, the upward half of a
//! failed or aborted decision, and the container join a settled child may
//! complete.

use super::*;
use crate::scheduler::shell::*;

impl KernelHost {
    /// Where the flow goes once `execution` settled, read from the declaring
    /// model: the node the flow enters next, and whether it must be re-created
    /// because its newest generation is already terminal (`planning::flow_target`
    /// for the model's own answer, a `while` step's loop-back for the loop).
    ///
    /// `None` for a node the model does not name as a step — a branch, a
    /// handler container, the workflow root — whose flow is the graph's own
    /// edges. A step its own guard skipped falls through the *declaration*
    /// chain, never its own `next`; a `while` step whose guard still holds
    /// after the iteration re-enters itself.
    async fn flow_of(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
    ) -> Result<Option<crate::scheduler::core::plan::propagation::FlowTarget>> {
        use crate::scheduler::core::plan::propagation::{FlowTarget, is_guard_skip};
        let Some(model) = model_of(snapshot)? else {
            return Ok(None);
        };
        let node = match snapshot.tasks.get(task_id) {
            Some(task) => task.model_node.as_str().to_string(),
            None => return Ok(None),
        };
        // Only a settled *success* flows on (a step its own guard skipped
        // included — that skip is the legacy fall-through). A failure, an
        // abort or a cascade skip owns the hop: its own walk decides the run,
        // and a flow from it would resurrect what the walk is ending.
        let state = snapshot.tasks.get(task_id).map(|task| task.state);
        if !matches!(
            state,
            Some(crate::scheduler::core::model::task_graph::TaskState::Completed)
                | Some(crate::scheduler::core::model::task_graph::TaskState::Submitted)
        ) && !is_guard_skip(snapshot, task_id)
        {
            return Ok(None);
        }
        if find_step_in_model(&model, &node).is_none() {
            return Ok(None);
        }
        let guard_skipped = is_guard_skip(snapshot, task_id);
        // What a re-entered generation's own scope must carry: its durable auto
        // decision and its declared model variables.
        let auto = auto_decision(snapshot, &node)?;
        let vars: Vec<(String, String)> = find_step_in_model(&model, &node)
            .map(|step| {
                step.vars
                    .iter()
                    .map(|var| (var.name.clone(), var.value.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        // A `while` step re-reads its guard after every iteration: while it
        // holds, the step runs again — the bounded loop the declaration
        // documents (`while` cannot be combined with `next`, checked at
        // import). The guard re-reads the state the iteration left, which the
        // completed execution's own scope carries.
        if !guard_skipped
            && let Some(node) = find_step_in_model(&model, &node)
            && let Some(expr) = node.r#while.as_ref()
        {
            let holds = self
                .evaluate_branch_predicate(snapshot, task_id, expr, &[])
                .await?;
            if holds == PredicateValue::True {
                return Ok(Some(FlowTarget {
                    node: node.id.clone(),
                    reopen: true,
                    auto,
                    vars,
                }));
            }
        }
        let Some(target) = flow_target(&model, &node, guard_skipped) else {
            return Ok(None);
        };
        // Whether the flow *admits* the target or re-enters it: a target whose
        // newest generation was never activated (`Created` — a step the flow
        // has not reached yet) is admitted in place; one that already ran, is
        // running or is waiting is re-entered as a fresh generation, because
        // the flow names the node's *next activation* (a backward `next`, a
        // step reached twice on different paths) and never the live one.
        let reopen = snapshot
            .tasks
            .iter()
            .filter(|(id, _)| id.node == target)
            .max_by_key(|(id, _)| id.generation)
            .is_some_and(|(_, task)| {
                task.state != crate::scheduler::core::model::task_graph::TaskState::Created
            });
        Ok(Some(FlowTarget {
            node: target,
            reopen,
            auto,
            vars,
        }))
    }

    /// The joined container's own completion message: a package-less
    /// container speaks here (its children settled and it reached its terminal
    /// state — the record is sequence-qualified, so a container that also
    /// announced an act keeps both). A `uses` step's act announced itself at
    /// the fan-out, so the join adds nothing for it: the legacy step's own
    /// messages are suppressed once it carries an act.
    fn attach_join_message(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        plan: &mut TransitionPlan,
        parent: &crate::scheduler::core::model::task_graph::TaskId,
        operation: &OperationIntent,
    ) -> Result<()> {
        let Some(task) = snapshot.tasks.get(parent) else {
            return Ok(());
        };
        if self.step_uses(snapshot, &task.model_node)?.is_some() {
            return Ok(());
        }
        if let Some(payload) =
            self.task_message_payload(snapshot, parent, &parent.node, None, None)?
        {
            plan.outbox.push(
                crate::scheduler::core::model::transition_plan::OutboxIntent {
                    key: format!(
                        "completed:{}:{}:{}:{}",
                        snapshot.process_id, parent.node, parent.generation, operation.id.sequence
                    ),
                    topic: "task.completed".into(),
                    payload: payload.to_string(),
                    receipt: None,
                },
            );
        }
        Ok(())
    }
}

/// The propagation hop: a settled execution carried into its successors.
pub(super) struct Propagate;

#[async_trait::async_trait]
impl OperationDriver for Propagate {
    /// Nothing to check before the claim: a settled execution always
    /// propagates.
    async fn precheck(&self, _host: &KernelHost, _pending: &Pending<'_>) -> Result<Precheck> {
        Ok(Precheck::Proceed)
    }

    /// Nothing runs: the plan is the whole hop.
    async fn run(&self, _host: &KernelHost, _hop: &Hop<'_>) -> Result<Outcome> {
        Ok(Outcome::Planned)
    }

    async fn next(
        &self,
        host: &KernelHost,
        hop: &Hop<'_>,
        _outcome: Outcome,
    ) -> Result<TransitionPlan> {
        let snapshot = hop.snapshot;
        let id = hop.id;
        let task_id = hop.task_id.clone();
        let claim = hop.claim;
        let task = hop.task.cloned();
        let plan = {
            // predecessor lists), never the declaring spec.
            let graph = task_graph(snapshot)?;
            let command = PropagateCommand {
                process_id: id.process_id.clone(),
                task: task_id.clone(),
                source_operation: claim.clone(),
                join_policy: crate::scheduler::core::plan::propagation::JoinPolicy::AllSucceeded,
                flow: host.flow_of(snapshot, &task_id).await?,
            };
            let mut plan = PropagationPlanner::plan(&graph, snapshot, &command).map_err(runtime)?;
            // A completed execution's own scope data folds into its parent
            // — the legacy `Task::next` → `parent.update_data(&outputs)`:
            // what a step's package wrote is what the step (and, up the
            // chain, the run) answers, which is what a workflow-level
            // `exposes` judges and what `proc:get`'s data shows. Engine
            // facts and anything this hop already writes for that scope
            // are left out (the failure path owns its own error
            // variables).
            if task.as_ref().map(|e| e.state)
                == Some(crate::scheduler::core::model::task_graph::TaskState::Completed)
                && let Some(parent) = task.as_ref().and_then(|e| e.parent.clone())
            {
                let mut carried: Vec<(String, String)> = Vec::new();
                for ((scope, name), raw) in &snapshot.variables {
                    if scope != &task_id || is_engine_fact(name) {
                        continue;
                    }
                    let already = plan
                        .variable_writes
                        .iter()
                        .any(|write| write.scope == parent && &write.name == name);
                    if !already && !carried.iter().any(|(existing, _)| existing == name) {
                        carried.push((name.clone(), raw.clone()));
                    }
                }
                for (name, value) in carried {
                    plan.variable_writes.push(VariableWrite {
                        scope: parent.clone(),
                        name,
                        value,
                    });
                }
            }
            // The decision walks' upward half: an aborted execution ends
            // its whole ancestor chain `Aborted` (legacy `abort_task`),
            // a failed one ends it `Failed` (legacy `emit_error` →
            // `parent.set_err`); each newly decided ancestor's own hop
            // carries the death into its remaining subtrees.
            let upward_to = match task.as_ref().map(|e| e.state) {
                Some(crate::scheduler::core::model::task_graph::TaskState::Aborted) => {
                    Some(crate::scheduler::core::model::task_graph::TaskState::Aborted)
                }
                Some(crate::scheduler::core::model::task_graph::TaskState::Failed) => {
                    Some(crate::scheduler::core::model::task_graph::TaskState::Failed)
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
                && let Some(parent) = task.as_ref().and_then(|e| e.parent.clone())
                // A hop that re-enters one of this parent's nodes (a
                // backward `next`, a `while` turn) is *not* the end of the
                // parent's chain: the join reads the snapshot, where the
                // re-entered child does not exist yet, so completing the
                // parent here would finish it under the node the flow just
                // created. Every other parent's join runs as usual (the
                // completed child's own branch/step settles with this hop).
                && !plan
                    .task_creates
                    .iter()
                    .any(|create| create.parent.as_ref() == Some(&parent))
            {
                let catch_join =
                    crate::scheduler::core::plan::container::ContainerPlanner::plan_catch_join(
                        &graph,
                        snapshot,
                        &crate::scheduler::core::plan::container::CatchJoinCommand {
                            process_id: id.process_id.clone(),
                            parent: parent.clone(),
                            source: task_id.clone(),
                        },
                    )
                    .map_err(runtime)?;
                match catch_join {
                    Some(crate::scheduler::core::plan::container::CatchJoinOutcome::Resumed(
                        change,
                        operation,
                    )) => {
                        // The record is keyed by the operation that decides
                        // the step — the durable-fact identity the commit
                        // requires (`failed:` keys never repeat).
                        let record_key = format!(
                            "failed:{}:{}:{}:{}",
                            snapshot.process_id,
                            parent.node,
                            parent.generation,
                            operation.id.sequence
                        );
                        plan.state_changes.push(change);
                        plan.operations.push(operation);
                        // The step's own failure message: the legacy walk
                        // re-derived the error and emitted the step's task
                        // event (`emit_error`), which is what tells the
                        // client the handlers caught nothing.
                        let code = task_variable(snapshot, &parent, "error.code")
                            .map(json_text)
                            .unwrap_or_default();
                        let text = task_variable(snapshot, &parent, "error.message")
                            .map(json_text)
                            .unwrap_or_default();
                        if let Some(payload) = host.hop_message_payload(
                            snapshot,
                            &parent,
                            &parent.node,
                            act_facts(snapshot, &parent).as_ref(),
                            None,
                            Some((&code, &text)),
                        )? {
                            plan.outbox.push(
                                crate::scheduler::core::model::transition_plan::OutboxIntent {
                                    key: record_key,
                                    topic: "task.failed".into(),
                                    payload: payload.to_string(),
                                    receipt: None,
                                },
                            );
                        }
                    }
                    Some(crate::scheduler::core::plan::container::CatchJoinOutcome::Caught(
                        change,
                        operation,
                    )) => {
                        host.attach_join_message(snapshot, &mut plan, &parent, &operation)?;
                        plan.state_changes.push(change);
                        plan.operations.push(operation);
                    }
                    None => {
                        let join = crate::scheduler::core::plan::container::ContainerJoinCommand {
                            process_id: id.process_id.clone(),
                            parent,
                        };
                        if let Some((change, operation)) =
                            crate::scheduler::core::plan::container::ContainerPlanner::plan_join(
                                &graph, snapshot, &join,
                            )
                            .map_err(runtime)?
                        {
                            host.attach_join_message(
                                snapshot,
                                &mut plan,
                                &join.parent,
                                &operation,
                            )?;
                            plan.state_changes.push(change);
                            plan.operations.push(operation);
                        }
                    }
                }
            }
            if let Some(to) = upward_to {
                let (changes, operations) =
                    crate::scheduler::core::plan::upward::UpwardPlanner::plan_upward(
                        snapshot,
                        &crate::scheduler::core::plan::upward::UpwardCommand {
                            process_id: id.process_id.clone(),
                            source: task_id.clone(),
                            to,
                        },
                    )
                    .map_err(runtime)?;
                // The error the decision carries travels up with it: the
                // legacy `emit_error` → `parent.set_err` wrote the same
                // code/message into every ancestor it decided, so an
                // ancestor's own handler reads `$ecode()`, and its error
                // row names what actually failed — instead of a bare
                // `Failed`. An abort carries no error text.
                if matches!(
                    to,
                    crate::scheduler::core::model::task_graph::TaskState::Failed
                ) {
                    let carried = ["error.code", "error.message", "ecode", "message"]
                        .into_iter()
                        .filter_map(|name| {
                            task_variable(snapshot, &task_id, name)
                                .map(|value| (name.to_string(), value.to_string()))
                        })
                        .collect::<Vec<_>>();
                    for change in &changes {
                        for (name, value) in &carried {
                            plan.variable_writes.push(VariableWrite {
                                scope: change.task.clone(),
                                name: name.clone(),
                                value: value.clone(),
                            });
                        }
                    }
                }
                plan.state_changes.extend(changes);
                plan.operations.extend(operations);
            }
            plan.validate().map_err(runtime)?;
            plan
        };
        Ok(plan)
    }
}
