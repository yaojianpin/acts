//! The candidates capability: the pool contract and its orchestration.
//!
//! The pool is computed locally when every atom of the declared expression
//! resolves in the kernel; a business atom makes it business data, so
//! computing it is one IRQ round trip, and the act's start must wait for it:
//! a container's fan-out (`in: "${{ candidates }}"`) and an owner expression
//! referencing the pool resolve *after* the answer lands. The shape is the
//! delegated join's ask: the waiting act pushes one request execution under
//! itself (marked [`CANDIDATES_WAIT_VAR`]/[`CANDIDATES_ASK_VAR`]), the client
//! answers `{ participants, ids, count }`, and the settlement folds the pool
//! into the act's variables and re-enters its start. A locally computed pool
//! folds through the same shape and re-enters the start without a request. A
//! failed request rides the ordinary error walk; a skipped one defers — the
//! re-entered start asks again.
//!
//! Every function here is a method on [`Assign`](super::Assign) — the same
//! type the pure semantics hang off in [`owner`](super::owner) and
//! [`delegate`](super::delegate).

use super::*;
use crate::{
    ActError,
    scheduler::{
        core::model::{
            operation::{OperationId, OperationKind},
            transition_plan::{OperationIntent, VariableWrite},
        },
        shell::{errors::runtime, planning::node_id},
    },
};

/// The private variable marking an act that waits for its candidate pool.
/// Its value is the pool spec's params (what the request carries), so a
/// replayed start re-derives the request without re-reading the model.
pub(crate) const CANDIDATES_WAIT_VAR: &str = "__candidates_wait";
/// The private variable marking a candidates request execution, and — on the
/// waiting act — the durable "pool computed" marker its re-entered start
/// reads (`__candidates` = the pool JSON).
pub(crate) const CANDIDATES_ASK_VAR: &str = "__candidates_ask";
pub(crate) const CANDIDATES_VAR: &str = "__candidates";

/// The pool spec one act declared: the request's params and its package.
#[derive(Clone, Debug)]
pub(crate) struct CandidatesSpec {
    pub(crate) params: crate::model::CandidateExpr,
    pub(crate) uses: String,
}

impl Assign {
    /// The `assign.candidates` spec one act declares, after refusing the
    /// declaration where it cannot mean a pool: a package-less node, the
    /// waiting subflow, or a `Func`/`Msg` leaf. Unlike `owner`, a pool is
    /// legitimate on the container packages (`block`/`parallel`/`sequence`) —
    /// the fan-out reads it. Pushed acts read the same declaration off their
    /// durable `__act` fact.
    pub(crate) fn declared_candidates(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        model_node: &crate::scheduler::core::model::model_graph::NodeId,
    ) -> crate::Result<Option<CandidatesSpec>> {
        let Some(model) = crate::scheduler::shell::model_of(snapshot)? else {
            return Ok(None);
        };
        let Some(step) = crate::scheduler::shell::find_step_in_model(&model, model_node.as_str())
        else {
            // A pushed act is not in the immutable model: its declaration
            // rides the execution's durable `__act` fact.
            return Self::pushed_declared_candidates(snapshot, task_id);
        };
        let Some(spec) = step
            .assign
            .as_ref()
            .and_then(|assign| assign.candidates.as_ref())
            .map(|candidates| CandidatesSpec {
                params: candidates.params.clone(),
                uses: candidates.uses.clone(),
            })
        else {
            return Ok(None);
        };
        // The placement is judged on the declaring act's own package — the
        // spec's `uses` names the pool's computing package, which is never a
        // container and would misjudge an auto act.
        let uses = step
            .uses
            .as_deref()
            .map(str::trim)
            .filter(|uses| !uses.is_empty())
            .ok_or_else(|| {
                crate::ActError::Action(format!(
                    "act '{}' declares assign.candidates but has no package",
                    task_id.node.as_str()
                ))
            })?;
        Self::reject_invalid_placement(snapshot, task_id.node.as_str(), uses)?;
        Ok(Some(spec))
    }

    /// The `assign.candidates` of a pushed act, refused on the same placements.
    fn pushed_declared_candidates(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
    ) -> crate::Result<Option<CandidatesSpec>> {
        let Some(act) = crate::scheduler::shell::action::pushed_act_of(snapshot, task_id)? else {
            return Ok(None);
        };
        let Some(assign) = &act.assign else {
            return Ok(None);
        };
        let Some(candidates) = &assign.candidates else {
            return Ok(None);
        };
        Self::reject_invalid_placement(snapshot, task_id.node.as_str(), &candidates.uses)?;
        Ok(Some(CandidatesSpec {
            params: candidates.params.clone(),
            uses: candidates.uses.clone(),
        }))
    }

    /// The placements no candidates spec can mean anything on: the waiting
    /// subflow, and a package whose run-bound self-completes (`Func`/`Msg` —
    /// it never waits for an answer) — except the container packages, which
    /// are auto *because* the kernel fans their children out itself, and
    /// whose fan-out is exactly what reads the pool.
    fn reject_invalid_placement(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        node: &str,
        uses: &str,
    ) -> crate::Result<()> {
        use crate::scheduler::shell::model::{BLOCK_PACKAGE, PARALLEL_PACKAGE, SEQUENCE_PACKAGE};
        if uses == crate::scheduler::shell::model::SUBFLOW_PACKAGE {
            return Err(crate::ActError::Action(format!(
                "act '{node}' uses '{uses}', which cannot declare candidates"
            )));
        }
        let container = matches!(uses, BLOCK_PACKAGE | PARALLEL_PACKAGE | SEQUENCE_PACKAGE);
        if crate::scheduler::shell::auto_decision(snapshot, node)? && !container {
            return Err(crate::ActError::Action(format!(
                "act '{node}' uses '{uses}', which cannot declare candidates (it never waits for an answer)"
            )));
        }
        Ok(())
    }

    /// Whether this execution's pool is already computed.
    pub(crate) fn pool_computed(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
    ) -> bool {
        crate::scheduler::shell::task_variable(snapshot, task_id, CANDIDATES_VAR).is_some()
    }

    /// Whether this execution is already waiting for a pool.
    pub(crate) fn pool_pending(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
    ) -> bool {
        crate::scheduler::shell::task_variable(snapshot, task_id, CANDIDATES_WAIT_VAR).is_some()
    }

    /// Push one candidates request under the waiting act: a `Ready` child
    /// with its start operation, riding the spec's package (`uses`) with the
    /// request's params (the serialized expression). The start announces the
    /// request's `Created` like any act's, which is what the client answers.
    pub(crate) fn ask(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        plan: &mut crate::scheduler::core::model::transition_plan::TransitionPlan,
        parent: &crate::scheduler::core::model::task_graph::TaskId,
        params: serde_json::Value,
        uses: String,
    ) -> crate::Result<()> {
        // The request node's id: `<act>#cand<k>`, unique against every
        // execution the run ever held, so a replay mints the same id
        // (idempotency) and re-asks never collide.
        let prefix = format!("{}#cand", parent.node.as_str());
        let nth = snapshot
            .tasks
            .keys()
            .filter(|existing| existing.node.as_str().starts_with(&prefix))
            .count()
            + 1;
        let node = format!("{prefix}{nth}");
        let ask =
            crate::scheduler::core::model::task_graph::TaskId::new(&node, 0).map_err(runtime)?;

        let act = crate::scheduler::shell::action::PushedAct {
            uses: uses.clone(),
            name: "candidates".to_string(),
            params,
            options: serde_json::Value::Null,
            exposes: Vec::new(),
            vars: Vec::new(),
            assign: None,
        };

        plan.variable_writes.push(VariableWrite {
            scope: ask.clone(),
            name: crate::scheduler::shell::action::PUSH_ACT_VAR.to_string(),
            value: act.fact(),
        });
        plan.variable_writes.push(VariableWrite {
            scope: ask.clone(),
            name: CANDIDATES_ASK_VAR.to_string(),
            value: uses,
        });
        // A request is always an interrupt (the client answers), so the
        // auto/msg maps gain the node with `false` — the lookups refuse a
        // missing entry.
        plan.variable_writes.push(VariableWrite {
            scope: ask.clone(),
            name: "auto".to_string(),
            value: "0".to_string(),
        });
        let root = crate::scheduler::core::decision::root_task(snapshot)
            .ok_or_else(|| ActError::Runtime("the run has no root".to_string()))?;
        for map_var in [
            crate::scheduler::shell::model::AUTO_MAP_VAR,
            crate::scheduler::shell::model::MSG_MAP_VAR,
        ] {
            let mut map: serde_json::Map<String, serde_json::Value> =
                crate::scheduler::shell::root_variable(snapshot, map_var)?
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

    /// One settlement under an act that waits for its pool: fold the
    /// request's answer into the act's variables (`candidates`, `ids`,
    /// `count`, the `__candidates` marker) and re-enter its start. A skipped
    /// request defers: the re-entered start asks again. A failed request is
    /// not ours — the ordinary error walk carries it. Returns whether the
    /// settlement was ours to handle.
    pub(crate) fn settle(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        plan: &mut crate::scheduler::core::model::transition_plan::TransitionPlan,
        parent: &crate::scheduler::core::model::task_graph::TaskId,
        settled: &crate::scheduler::core::model::task_graph::TaskId,
    ) -> crate::Result<bool> {
        // Only an act that waits for its pool settles a candidates request:
        // every other parent/child pair is the ordinary walk's business.
        if !Self::pool_pending(snapshot, parent) {
            return Ok(false);
        }
        if crate::scheduler::shell::task_variable(snapshot, settled, CANDIDATES_ASK_VAR).is_none() {
            return Ok(false);
        }
        let state = snapshot
            .tasks
            .get(settled)
            .map(|task| task.state)
            .ok_or_else(|| runtime("the settled candidates request vanished"))?;
        match state {
            crate::scheduler::core::model::task_graph::TaskState::Completed => {
                // The answer's contract: `{ participants, ids, count }` —
                // the pool the fan-out reads is the **ids list** (`in:
                // "${{ candidates }}"`, `owner: "${{ candidates[0] }}"`);
                // `participants`/`count` ride along for the flows that want
                // code/name or a quorum number. The propagate's carry
                // already folds the request's own variables into the act's
                // scope; this writes the canonical pool and the computed
                // marker, skipping duplicates so the plan stays valid.
                let participants = Self::answer_var(snapshot, settled, "participants");
                let ids = match Self::answer_var(snapshot, settled, "ids") {
                    serde_json::Value::Array(_) => Self::answer_var(snapshot, settled, "ids"),
                    _ => serde_json::Value::Array(
                        participants
                            .as_array()
                            .map(|list| {
                                list.iter()
                                    .filter_map(|entry| {
                                        entry.get("id").and_then(|id| id.as_str()).map(String::from)
                                    })
                                    .map(serde_json::Value::String)
                                    .collect()
                            })
                            .unwrap_or_default(),
                    ),
                };
                let count = Self::answer_var(snapshot, settled, "count");
                let computed = serde_json::json!({
                    "participants": participants,
                    "ids": ids,
                    "count": count,
                });
                Self::fold_pool(plan, parent, computed);
            }
            crate::scheduler::core::model::task_graph::TaskState::Skipped => {
                // Defer: the re-entered start sees the wait marker with no
                // pool and asks again (the client is the decider, exactly
                // like a delegated join's re-ask loop).
            }
            _ => return Ok(false),
        }
        plan.operations.push(OperationIntent {
            id: OperationId {
                process_id: snapshot.process_id.clone(),
                task: parent.clone(),
                sequence: snapshot.next_operation_sequence(parent.node.as_str(), parent.generation),
            },
            kind: OperationKind::ExecuteTask,
            ready_at_ms: None,
        });
        Ok(true)
    }

    /// The pool an expression whose every atom resolved in the kernel
    /// produced: fold it into the declaring act's variables — the same shape
    /// a settled answer lands in — and re-enter its start, which finds the
    /// pool in scope and proceeds without a round trip.
    pub(crate) fn settle_local_pool(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        plan: &mut crate::scheduler::core::model::transition_plan::TransitionPlan,
        parent: &crate::scheduler::core::model::task_graph::TaskId,
        participants: &[crate::model::Participant],
    ) -> crate::Result<()> {
        let ids = serde_json::Value::Array(
            participants
                .iter()
                .map(|participant| serde_json::Value::String(participant.id.clone()))
                .collect(),
        );
        let count = serde_json::json!(participants.len());
        let participants = serde_json::to_value(participants).unwrap_or(serde_json::Value::Null);
        let computed = serde_json::json!({
            "participants": participants,
            "ids": ids,
            "count": count,
        });
        Self::fold_pool(plan, parent, computed);
        plan.operations.push(OperationIntent {
            id: OperationId {
                process_id: snapshot.process_id.clone(),
                task: parent.clone(),
                sequence: snapshot.next_operation_sequence(parent.node.as_str(), parent.generation),
            },
            kind: OperationKind::ExecuteTask,
            ready_at_ms: None,
        });
        Ok(())
    }

    /// Write one computed pool into the act's variables: `candidates` (the
    /// ids list the fan-out and owner expressions read), the
    /// `participants`/`count` answer fields, the durable `__candidates`
    /// marker the re-entered start reads as "pool computed", and the wait
    /// marker cleared. Writes the answer fields only when the plan does not
    /// already carry them (a settled request's propagate folds the request's
    /// own variables into the act's scope first).
    fn fold_pool(
        plan: &mut crate::scheduler::core::model::transition_plan::TransitionPlan,
        parent: &crate::scheduler::core::model::task_graph::TaskId,
        computed: serde_json::Value,
    ) {
        let ids = computed
            .get("ids")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let mut writes = vec![
            (
                "candidates".to_string(),
                serde_json::to_string(&ids).unwrap_or_default(),
            ),
            (CANDIDATES_VAR.to_string(), computed.to_string()),
            (CANDIDATES_WAIT_VAR.to_string(), "null".to_string()),
        ];
        for name in ["participants", "count"] {
            let value = computed
                .get(name)
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let carried = plan
                .variable_writes
                .iter()
                .any(|write| write.scope == *parent && write.name == name);
            if !carried {
                writes.push((
                    name.to_string(),
                    serde_json::to_string(&value).unwrap_or_default(),
                ));
            }
        }
        for (name, value) in writes {
            plan.variable_writes.push(VariableWrite {
                scope: parent.clone(),
                name,
                value,
            });
        }
    }

    /// One answer field off the request's outputs, best effort: a missing or
    /// non-array field is an empty pool (the business system answered; it owns
    /// the shape).
    fn answer_var(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        settled: &crate::scheduler::core::model::task_graph::TaskId,
        name: &str,
    ) -> serde_json::Value {
        crate::scheduler::shell::task_variable(snapshot, settled, name)
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
            .unwrap_or(serde_json::Value::Null)
    }
}
