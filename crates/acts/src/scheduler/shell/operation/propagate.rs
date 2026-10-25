//! The `PropagateTask` operation: carrying a settled execution into its successors.
//!
//! The operation runs nothing (phase 2 is the snapshot itself); phase 3 re-plans
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
        // abort or a cascade skip owns the operation: its own walk decides the run,
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
            .filter(|(id, _)| id.node.as_str() == target)
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
    pub(super) fn attach_join_message(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        plan: &mut TransitionPlan,
        parent: &crate::scheduler::core::model::task_graph::TaskId,
        operation: &OperationIntent,
    ) -> Result<()> {
        let Some(task) = snapshot.tasks.get(parent) else {
            return Ok(());
        };
        if self
            .step_uses(snapshot, parent, &task.model_node)?
            .is_some()
        {
            return Ok(());
        }
        if let Some(payload) =
            self.task_message_payload(snapshot, parent, parent.node.as_str(), None, None)?
        {
            plan.outbox.push(
                crate::scheduler::core::model::transition_plan::OutboxIntent {
                    key: format!(
                        "completed:{}:{}:{}:{}",
                        snapshot.process_id,
                        parent.node.as_str(),
                        parent.generation,
                        operation.id.sequence
                    ),
                    topic: "task.completed".into(),
                    payload: payload.to_string(),
                },
            );
        }
        Ok(())
    }
}

/// The propagation operation: a settled execution carried into its successors.
pub(super) struct PropagateTask;

#[async_trait::async_trait]
impl ActOperation for PropagateTask {
    /// Nothing to check before the claim: a settled execution always
    /// propagates.
    async fn precheck(&self, _host: &KernelHost, _pending: &Pending<'_>) -> Result<Precheck> {
        Ok(Precheck::Proceed)
    }

    /// Nothing runs: the plan is the whole operation.
    async fn run(&self, _host: &KernelHost, _operation: &mut Operation<'_>) -> Result<Outcome> {
        Ok(Outcome::Planned)
    }

    async fn next(
        &self,
        host: &KernelHost,
        operation: &Operation<'_>,
        _outcome: Outcome,
    ) -> Result<TransitionPlan> {
        let snapshot = operation.snapshot();
        let id = operation.id;
        let task_id = operation.task_id.clone();
        let claim = operation.claim;
        let task = operation.task().cloned();
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
            let mut plan = Propagate::plan((&graph, snapshot), &command).map_err(runtime)?;
            // A completed execution's own scope data folds into its parent
            // — the legacy `Task::next` → `parent.update_data(&outputs)`:
            // what a step's package wrote is what the step (and, up the
            // chain, the run) answers, which is what a workflow-level
            // `exposes` judges and what `proc:get`'s data shows. Engine
            // facts and anything this operation already writes for that scope
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
            // `parent.set_err`); each newly decided ancestor's own operation
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
            // The error walk stops at the nearest join-rule container that
            // tolerates failure: the failure lands on it as one
            // non-completion in its rule's census, not as its own death. The
            // walk still decides everything below that container (a failed
            // vote fails its own branch), and the legacy walk keeps its shape
            // wherever no tolerant rule absorbs the death. The abort walk
            // never stops.
            let mut walk_stop = None;
            if upward_to == Some(crate::scheduler::core::model::task_graph::TaskState::Failed) {
                let mut cursor = task.as_ref().and_then(|e| e.parent.clone());
                while let Some(ancestor) = cursor {
                    let Some(ancestor_task) = snapshot.tasks.get(&ancestor) else {
                        break;
                    };
                    let tolerant = crate::scheduler::core::plan::container::parked_for_children(
                        snapshot, &ancestor,
                    ) && container_join(snapshot, &ancestor)?
                        .as_ref()
                        .is_some_and(crate::JoinRule::tolerates_failure);
                    if tolerant {
                        walk_stop = Some(ancestor);
                        break;
                    }
                    cursor = ancestor_task.parent.clone();
                }
            }
            // The container join: a parent parked by a fan-out completes
            // in the same atomic commit that settles its last child; a
            // failed step's CATCH child runs the catch join instead, which
            // either catches the error or resumes it. A decision walk
            // already decides the parent itself (legacy calls
            // `parent.set_err` before the auto-complete pass sees the
            // children settled), so both yield to it — the same change
            // twice is not a plan. The one exception is the death a
            // tolerant rule absorbed: the walk stopped below the parent
            // (it is the walk's bound), and the parent decides by its rule.
            let rule_arm = walk_stop
                .as_ref()
                .is_some_and(|stop| Some(stop) == task.as_ref().and_then(|e| e.parent.as_ref()));
            if (upward_to.is_none() || rule_arm)
                && let Some(parent) = task.as_ref().and_then(|e| e.parent.clone())
                // An operation that re-enters one of this parent's nodes (a
                // backward `next`, a `while` turn) is *not* the end of the
                // parent's chain: the join reads the snapshot, where the
                // re-entered child does not exist yet, so completing the
                // parent here would finish it under the node the flow just
                // created. Every other parent's join runs as usual (the
                // completed child's own branch/step settles with this operation).
                && !plan
                    .task_creates
                    .iter()
                    .any(|create| create.parent.as_ref() == Some(&parent))
            {
                let catch_join = CatchJoin::plan(
                    (&graph, snapshot),
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
                            parent.node.as_str(),
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
                        if let Some(payload) = host.operation_message_payload(
                            snapshot,
                            &parent,
                            parent.node.as_str(),
                            act_facts(snapshot, &parent).as_ref(),
                            None,
                            Some((&code, &text)),
                        )? {
                            plan.outbox.push(
                                crate::scheduler::core::model::transition_plan::OutboxIntent {
                                    key: record_key,
                                    topic: "task.failed".into(),
                                    payload: payload.to_string(),
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
                        // The parent's join rule, when the parent is a
                        // container parked for its children: it decides the
                        // join, and a delegated rule turns the settlement
                        // into a decision request instead of a count.
                        let join = if crate::scheduler::core::plan::container::parked_for_children(
                            snapshot, &parent,
                        ) {
                            container_join(snapshot, &parent)?
                        } else {
                            None
                        };
                        if let Some(delegated @ crate::JoinRule::Action { .. }) = join.as_ref() {
                            host.join_settlement(
                                snapshot, &mut plan, &parent, &task_id, delegated,
                            )?;
                        }
                        let command =
                            crate::scheduler::core::plan::container::ContainerJoinCommand {
                                process_id: id.process_id.clone(),
                                parent: parent.clone(),
                                join,
                            };
                        if let Some(outcome) =
                            Join::plan((&graph, snapshot), &command).map_err(runtime)?
                        {
                            if let Some((code, text)) = &outcome.failure {
                                // The rule's failure is the container's own:
                                // the error variables a catch predicate reads
                                // (`$ecode()`/`message` and the kernel's own
                                // pair), and the step's task event.
                                for (name, value) in [
                                    ("error.code", code.as_str()),
                                    ("error.message", text.as_str()),
                                    ("ecode", code.as_str()),
                                    ("message", text.as_str()),
                                ] {
                                    plan.variable_writes.push(VariableWrite {
                                        scope: parent.clone(),
                                        name: name.to_string(),
                                        value: json_var(value),
                                    });
                                }
                                if let Some(payload) = host.operation_message_payload(
                                    snapshot,
                                    &parent,
                                    parent.node.as_str(),
                                    act_facts(snapshot, &parent).as_ref(),
                                    None,
                                    Some((code.as_str(), text.as_str())),
                                )? {
                                    plan.outbox.push(
                                        crate::scheduler::core::model::transition_plan::OutboxIntent {
                                            key: format!(
                                                "failed:{}:{}:{}:{}",
                                                snapshot.process_id,
                                                parent.node.as_str(),
                                                parent.generation,
                                                outcome.operation.id.sequence
                                            ),
                                            topic: "task.failed".into(),
                                            payload: payload.to_string(),
                                        },
                                    );
                                }
                            } else {
                                host.attach_join_message(
                                    snapshot,
                                    &mut plan,
                                    &command.parent,
                                    &outcome.operation,
                                )?;
                            }
                            plan.state_changes.push(outcome.change);
                            plan.operations.push(outcome.operation);
                            plan.operations.extend(outcome.cancels);
                        }
                    }
                }
            }
            if let Some(to) = upward_to {
                let (changes, operations) = Upward::plan(
                    snapshot,
                    &crate::scheduler::core::plan::upward::UpwardCommand {
                        process_id: id.process_id.clone(),
                        source: task_id.clone(),
                        to,
                        stop: walk_stop,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::core::model::task_graph::{TaskId, TaskState};

    fn host() -> std::sync::Arc<KernelHost> {
        KernelHost::in_memory().0
    }

    fn id(node: &str) -> TaskId {
        TaskId::new(node, 0).unwrap()
    }

    fn intent(sequence: u64) -> OperationIntent {
        OperationIntent {
            id: OperationId {
                process_id: "p".into(),
                task: id("s1"),
                sequence,
            },
            kind: OperationKind::PropagateTask,
            ready_at_ms: None,
        }
    }

    /// A flow only exists when the aggregate carries a declaring model; an
    /// aggregate seeded before the model fact has none.
    #[tokio::test]
    async fn no_model_means_no_declared_flow() {
        let host = host();
        let mut snapshot = ProcessSpec::linear("p", "root", &["a"])
            .seed_snapshot()
            .unwrap();
        let root = id("root");
        snapshot
            .variables
            .remove(&(root.clone(), super::super::model::MODEL_VAR.to_string()));
        assert!(host.flow_of(&snapshot, &root).await.unwrap().is_none());
    }

    /// A `while` whose guard still holds after the iteration re-enters the
    /// node instead of following its declared next.
    #[tokio::test]
    async fn a_holding_while_reopens_the_node() {
        let host = host();
        let workflow =
            crate::model::Workflow::new().with_step(|step| step.with_id("s1").with_while("true"));
        let spec = ProcessSpec::from_workflow("p", &workflow, &|_| None).unwrap();
        let mut snapshot = spec.seed_snapshot().unwrap();
        let source = id("s1");
        snapshot.tasks.get_mut(&source).unwrap().state = TaskState::Completed;
        let target = host
            .flow_of(&snapshot, &source)
            .await
            .unwrap()
            .expect("a flow");
        assert_eq!(target.node, "s1");
        assert!(target.reopen, "a loop turn is a fresh generation");
    }

    /// A join for an execution the aggregate no longer holds says nothing.
    #[tokio::test]
    async fn a_join_message_for_a_missing_parent_is_ignored() {
        let host = host();
        let snapshot = ProcessSpec::linear("p", "root", &["a"])
            .seed_snapshot()
            .unwrap();
        let mut plan = TransitionPlan {
            process_id: "p".into(),
            expected: snapshot.version.clone(),
            task_creates: Vec::new(),
            state_changes: Vec::new(),
            variable_writes: Vec::new(),
            source_operation: None,
            operations: Vec::new(),
            outbox: Vec::new(),
        };
        host.attach_join_message(&snapshot, &mut plan, &id("ghost"), &intent(3))
            .unwrap();
        assert!(plan.outbox.is_empty());
    }

    /// A package-less container speaks its own completion at the join.
    #[tokio::test]
    async fn a_package_less_parent_announces_its_join() {
        let host = host();
        let workflow = crate::model::Workflow::new().with_step(|step| step.with_id("a"));
        let spec = ProcessSpec::from_workflow("p", &workflow, &|_| None).unwrap();
        let snapshot = spec.seed_snapshot().unwrap();
        let mut plan = TransitionPlan {
            process_id: "p".into(),
            expected: snapshot.version.clone(),
            task_creates: Vec::new(),
            state_changes: Vec::new(),
            variable_writes: Vec::new(),
            source_operation: None,
            operations: Vec::new(),
            outbox: Vec::new(),
        };
        host.attach_join_message(&snapshot, &mut plan, &id("a"), &intent(4))
            .unwrap();
        assert_eq!(plan.outbox.len(), 1);
        assert_eq!(plan.outbox[0].topic, "task.completed");
    }

    /// A `uses` container announced its act at the fan-out; the join adds
    /// nothing for it.
    #[tokio::test]
    async fn a_package_parent_suppresses_the_join_message() {
        let host = host();
        let workflow = crate::model::Workflow::new()
            .with_step(|step| step.with_id("s1").with_uses("pkg.test", crate::Vars::new()));
        let spec = ProcessSpec::from_workflow("p", &workflow, &|_| None).unwrap();
        let snapshot = spec.seed_snapshot().unwrap();
        let mut plan = TransitionPlan {
            process_id: "p".into(),
            expected: snapshot.version.clone(),
            task_creates: Vec::new(),
            state_changes: Vec::new(),
            variable_writes: Vec::new(),
            source_operation: None,
            operations: Vec::new(),
            outbox: Vec::new(),
        };
        host.attach_join_message(&snapshot, &mut plan, &id("s1"), &intent(5))
            .unwrap();
        assert!(plan.outbox.is_empty());
    }
}
