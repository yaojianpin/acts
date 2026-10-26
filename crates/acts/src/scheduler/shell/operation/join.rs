//! The delegated join rule (`rule: { action, params }`).
//!
//! A container whose rule is delegated decides by the client's verdict, not
//! by a count: on *every* child settlement the engine asks — it creates one
//! `acts.core.join` request under the container, carrying the rule's key, its
//! params and the children's outcomes census — and the client answers
//!
//! - `complete` — the container completes (the stragglers' votes are
//!   withdrawn: the live children and any other pending request skip);
//! - `fail` — the container fails with the answer's error, its own
//!   propagation carrying the failure up and skipping the stragglers;
//! - `skip` — defer: the request settles skipped; the next settlement asks
//!   again, and once every child settled with no request live, the engine
//!   asks once more (the client is the decider, so the re-ask loop is its
//!   own choice — a stuck client is a stuck container, exactly like an
//!   unanswered act).
//!
//! The requests are children the model does not declare: they ride their own
//! executions like a pushed act (`PUSH_ACT_VAR`), are marked
//! [`JOIN_ASK_VAR`] so the join counts real children only, and are cancelled
//! with everything else when a verdict lands.

use crate::scheduler::core::model::transition_plan::OutboxIntent;
use crate::scheduler::shell::*;

/// The private variable marking a join-decision request execution. The
/// container's join counts its real children only, and a settled request's
/// own state is the verdict: completed = pass, failed = reject, skipped =
/// defer. A `__` fact, like every engine fact the operations address each
/// other with.
pub(super) const JOIN_ASK_VAR: &str = "__join_ask";

impl KernelHost {
    /// One child of a delegated-rule container settled: either it carries the
    /// client's verdict (a request the client answered) or the settlement
    /// asks for one.
    pub(super) fn join_settlement(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        plan: &mut crate::scheduler::core::model::transition_plan::TransitionPlan,
        parent: &crate::scheduler::core::model::task_graph::TaskId,
        settled: &crate::scheduler::core::model::task_graph::TaskId,
        rule: &crate::JoinRule,
    ) -> Result<()> {
        // A container that already decided (a racing verdict, a replayed
        // propagation) or no longer parks decides nothing.
        let Some(parent_task) = snapshot.tasks.get(parent) else {
            return Ok(());
        };
        if parent_task.state != crate::scheduler::core::model::task_graph::TaskState::Running
            || !crate::scheduler::core::plan::container::parked_for_children(snapshot, parent)
        {
            return Ok(());
        }
        if task_variable(snapshot, settled, JOIN_ASK_VAR).is_some() {
            // The verdict the client answered the request with.
            let state = snapshot
                .tasks
                .get(settled)
                .map(|task| task.state)
                .unwrap_or(crate::scheduler::core::model::task_graph::TaskState::Skipped);
            match state {
                crate::scheduler::core::model::task_graph::TaskState::Completed => {
                    self.join_decide(
                        snapshot,
                        plan,
                        parent,
                        crate::scheduler::core::model::task_graph::TaskState::Completed,
                        None,
                    )?;
                }
                crate::scheduler::core::model::task_graph::TaskState::Failed => {
                    // The rejection's own error is the container's: the
                    // client failed the request with a code and a message,
                    // and the run should see exactly those.
                    let code = task_variable(snapshot, settled, "error.code")
                        .map(json_text)
                        .unwrap_or_default();
                    let message = task_variable(snapshot, settled, "error.message")
                        .map(json_text)
                        .unwrap_or_default();
                    self.join_decide(
                        snapshot,
                        plan,
                        parent,
                        crate::scheduler::core::model::task_graph::TaskState::Failed,
                        Some((code, message)),
                    )?;
                }
                // Defer. A skipped request asks again only when nothing else
                // is coming: every real child settled and no other request
                // is live.
                crate::scheduler::core::model::task_graph::TaskState::Skipped
                    if !Self::join_has_live_work(snapshot, parent, settled)? =>
                {
                    self.join_ask(snapshot, plan, parent, rule)?;
                }
                _ => {}
            }
        } else {
            // A real child settled: ask for a verdict over the fresh census.
            self.join_ask(snapshot, plan, parent, rule)?;
        }
        Ok(())
    }

    /// Whether the container still holds work other than `settled` — a real
    /// child in flight (whose settlement will ask) or another live request
    /// (the client still owes it an answer).
    fn join_has_live_work(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        parent: &crate::scheduler::core::model::task_graph::TaskId,
        settled: &crate::scheduler::core::model::task_graph::TaskId,
    ) -> Result<bool> {
        let graph = task_graph(snapshot)?;
        for child in graph.children(parent) {
            if *child == *settled {
                continue;
            }
            let Some(task) = snapshot.tasks.get(child) else {
                continue;
            };
            if !task.state.is_terminal() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The outcomes census one delegated container's request carries: every
    /// counted child with its node, generation, state and user outputs —
    /// what a client handler needs to decide ("the chair approved" is one
    /// entry's outputs). Requests themselves are not votes.
    fn join_outcomes(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        parent: &crate::scheduler::core::model::task_graph::TaskId,
    ) -> Result<serde_json::Value> {
        let graph = task_graph(snapshot)?;
        let mut outcomes = Vec::new();
        for child in graph.children(parent) {
            if task_variable(snapshot, child, JOIN_ASK_VAR).is_some() {
                continue;
            }
            let Some(task) = snapshot.tasks.get(child) else {
                continue;
            };
            let outputs = root_variables(snapshot, child, |name| !is_error_fact(name));
            outcomes.push(serde_json::json!({
                "node": child.node.as_str(),
                "generation": child.generation,
                "state": task.state.as_str(),
                "outputs": serde_json::to_value(&outputs).unwrap_or(serde_json::Value::Null),
            }));
        }
        Ok(serde_json::Value::Array(outcomes))
    }

    /// Create one `acts.core.join` request under the container: a `Ready`
    /// child with its start operation, whose facts ride its own scope (the
    /// model is immutable), marked so the join and the verdict reader
    /// recognize it. The start announces the request's `Created` like any
    /// act's, which is what the client answers.
    fn join_ask(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        plan: &mut crate::scheduler::core::model::transition_plan::TransitionPlan,
        parent: &crate::scheduler::core::model::task_graph::TaskId,
        rule: &crate::JoinRule,
    ) -> Result<()> {
        let crate::JoinRule::Action { action, params } = rule else {
            return Ok(());
        };
        // The request node's id: `<container>#rule<k>`, unique against every
        // execution the run ever held, so a replay against the same snapshot
        // mints the same id (idempotency) and two asks never collide.
        let prefix = format!("{}#rule", parent.node.as_str());
        let nth = snapshot
            .tasks
            .keys()
            .filter(|existing| existing.node.as_str().starts_with(&prefix))
            .count()
            + 1;
        let node = format!("{prefix}{nth}");
        let ask =
            crate::scheduler::core::model::task_graph::TaskId::new(&node, 0).map_err(runtime)?;

        let outcomes = Self::join_outcomes(snapshot, parent)?;
        let act = crate::scheduler::shell::action::PushedAct {
            uses: crate::scheduler::shell::model::DECIDE_PACKAGE.to_string(),
            name: action.clone(),
            params: serde_json::json!({
                "action": action,
                "params": params,
                "outcomes": outcomes,
            }),
            options: serde_json::Value::Null,
            exposes: Vec::new(),
            vars: Vec::new(),
        };

        // The run-bound decisions the import built for declared nodes; a
        // request is always an interrupt (the client decides), so both maps
        // gain the node with `false` — the lookups refuse a missing entry.
        let root = crate::scheduler::core::decision::root_task(snapshot)
            .ok_or_else(|| ActError::Runtime("the run has no root".to_string()))?;
        plan.variable_writes.push(VariableWrite {
            scope: ask.clone(),
            name: crate::scheduler::shell::action::PUSH_ACT_VAR.to_string(),
            value: act.fact(),
        });
        plan.variable_writes.push(VariableWrite {
            scope: ask.clone(),
            name: JOIN_ASK_VAR.to_string(),
            value: action.clone(),
        });
        plan.variable_writes.push(VariableWrite {
            scope: ask.clone(),
            name: "auto".to_string(),
            value: "0".to_string(),
        });
        for map_var in [
            crate::scheduler::shell::model::AUTO_MAP_VAR,
            crate::scheduler::shell::model::MSG_MAP_VAR,
        ] {
            let mut map: serde_json::Map<String, serde_json::Value> =
                root_variable(snapshot, map_var)?
                    .map(|raw| serde_json::from_str(&raw))
                    .transpose()
                    .map_err(runtime)?
                    .unwrap_or_default();
            map.insert(node.clone(), serde_json::Value::Bool(false));
            plan.variable_writes.push(VariableWrite {
                scope: root.clone(),
                name: map_var.to_string(),
                value: serde_json::Value::Object(map).to_string(),
            });
        }

        plan.task_creates
            .push(crate::scheduler::core::model::transition_plan::TaskCreate {
                task: ask.clone(),
                model_node: node_id(&node)?,
                parent: Some(parent.clone()),
                predecessors: Vec::new(),
                initial_state: crate::scheduler::core::model::task_graph::TaskState::Ready,
            });
        plan.operations.push(OperationIntent {
            id: OperationId {
                process_id: snapshot.process_id.clone(),
                task: ask,
                sequence: snapshot.next_operation_sequence(node.as_str(), 0),
            },
            kind: OperationKind::ExecuteTask,
            ready_at_ms: None,
        });
        Ok(())
    }

    /// Apply the client's verdict to the container: the state change, the
    /// propagation (whose error walk carries a failure up and whose
    /// dead-source cascade skips the stragglers), and the message.
    fn join_decide(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        plan: &mut crate::scheduler::core::model::transition_plan::TransitionPlan,
        parent: &crate::scheduler::core::model::task_graph::TaskId,
        to: crate::scheduler::core::model::task_graph::TaskState,
        failure: Option<(String, String)>,
    ) -> Result<()> {
        let parent_task = snapshot
            .tasks
            .get(parent)
            .ok_or_else(|| runtime(format!("execution {parent:?} not found")))?;
        let sequence = snapshot
            .operations
            .keys()
            .filter(|id| id.process_id == snapshot.process_id && id.task == *parent)
            .map(|id| id.sequence)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| runtime("operation sequence overflow".to_string()))?;
        plan.state_changes.push(StateChange {
            task: parent.clone(),
            expected_revision: parent_task.revision,
            from: crate::scheduler::core::model::task_graph::TaskState::Running,
            to,
        });
        let propagate = OperationIntent {
            id: OperationId {
                process_id: snapshot.process_id.clone(),
                task: parent.clone(),
                sequence,
            },
            kind: OperationKind::PropagateTask,
            ready_at_ms: None,
        };
        match &failure {
            Some((code, message)) => {
                for (name, value) in [
                    ("error.code", code),
                    ("error.message", message),
                    ("ecode", code),
                    ("message", message),
                ] {
                    plan.variable_writes.push(VariableWrite {
                        scope: parent.clone(),
                        name: name.to_string(),
                        value: json_var(value),
                    });
                }
                if let Some(payload) = self.operation_message_payload(
                    snapshot,
                    parent,
                    parent.node.as_str(),
                    act_facts(snapshot, parent).as_ref(),
                    None,
                    Some((code.as_str(), message.as_str())),
                )? {
                    plan.outbox.push(OutboxIntent {
                        key: format!(
                            "failed:{}:{}:{}:{}",
                            snapshot.process_id,
                            parent.node.as_str(),
                            parent.generation,
                            sequence
                        ),
                        topic: "task.failed".into(),
                        payload: payload.to_string(),
                    });
                }
            }
            None => {
                // A completion cancels the stragglers itself: its propagation
                // is alive, so nothing else withdraws the votes still out.
                // `SkipTask` settles admitted executions only (a container's
                // children always are — its own fan-out admitted them).
                let graph = task_graph(snapshot)?;
                for child in graph.children(parent) {
                    let Some(child_task) = snapshot.tasks.get(child) else {
                        continue;
                    };
                    if !matches!(
                        child_task.state,
                        crate::scheduler::core::model::task_graph::TaskState::Ready
                            | crate::scheduler::core::model::task_graph::TaskState::Running
                            | crate::scheduler::core::model::task_graph::TaskState::Waiting
                    ) {
                        continue;
                    }
                    plan.operations.push(OperationIntent {
                        id: OperationId {
                            process_id: snapshot.process_id.clone(),
                            sequence: snapshot
                                .next_operation_sequence(child.node.as_str(), child.generation),
                            task: child.clone(),
                        },
                        kind: OperationKind::SkipTask,
                        ready_at_ms: None,
                    });
                }
                self.attach_join_message(snapshot, plan, parent, &propagate)?;
            }
        }
        plan.operations.push(propagate);
        Ok(())
    }
}
