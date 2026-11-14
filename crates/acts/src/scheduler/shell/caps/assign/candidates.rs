//! The candidates capability: the pool contract and its orchestration.
//!
//! The pool is computed locally when every atom of the declared expression
//! resolves in the kernel; a business atom makes it business data, so
//! computing it is one IRQ round trip, and the act's start must wait for it:
//! a container's fan-out (`in: "${{ @candidates }}"`) and an owner expression
//! referencing the pool resolve *after* the answer lands. The shape is the
//! delegated join's ask: the waiting act pushes one request execution under
//! itself (marked [`CANDIDATES_WAIT_VAR`]/[`CANDIDATES_ASK_VAR`]), the client
//! answers `{ participants, ids, count }`, and the settlement folds the pool
//! into the act's variables and re-enters its start. A locally computed pool
//! folds through the same shape and re-enters the start without a request. A
//! failed request rides the ordinary error walk; a skipped one defers — the
//! re-entered start asks again.
//!
//! Beyond the canonical `assign.candidates` pool, every other key of the
//! declaration is a named pool processed exactly the same way: it gets its
//! own request, its own computed/wait markers (`#pool_<name>` /
//! `#pool_wait_<name>`), and its answer lands under its own key
//! (`$assign.<name>.users` / `.ids`, the bare `<name>` ids variable). A start
//! with several pools settles them one re-entry at a time; it proceeds only
//! when every declared pool is computed.
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
pub(crate) const CANDIDATES_WAIT_VAR: &str = "#candidates_wait";
/// The private variable marking a candidates request execution, and — on the
/// waiting act — the durable "pool computed" marker its re-entered start
/// reads (`#candidates` = the pool JSON).
pub(crate) const CANDIDATES_ASK_VAR: &str = "#candidates_ask";
pub(crate) const CANDIDATES_VAR: &str = "#candidates";

/// The declaration key of the canonical pool. Every other key of `assign` is
/// a named pool with the same processing; a named pool's markers and answer
/// live under its own name.
pub(crate) const CANONICAL_POOL: &str = "candidates";

/// The names no pool key may take: `assignee`/`delegator` are the `$assign`
/// projection's identity fields, `ids`/`participants`/`count` the canonical
/// pool's top-level aliases, and `at` its explicit lookup — a pool under such
/// a key would be unreadable. `owner`/`candidates` need no reservation: the
/// declaration routes them to their named fields, and the inherited canonical
/// key is `candidates` itself.
const RESERVED_POOL_NAMES: [&str; 6] = [
    "assignee",
    "delegator",
    "ids",
    "participants",
    "count",
    "at",
];

/// The pool spec one act declared: its declaration key, the request's params
/// and its package.
#[derive(Clone, Debug)]
pub(crate) struct CandidatesSpec {
    /// The declaration key: [`CANONICAL_POOL`] or one of `assign.others`.
    pub(crate) pool: String,
    pub(crate) params: crate::model::CandidateExpr,
    pub(crate) uses: String,
}

impl Assign {
    /// The computed-pool marker variable of one pool: the canonical pool stays
    /// on `#candidates`; a named pool gets `#pool_<name>`.
    pub(crate) fn pool_var(pool: &str) -> String {
        if pool == CANONICAL_POOL {
            CANDIDATES_VAR.to_string()
        } else {
            format!("#pool_{pool}")
        }
    }

    /// The wait-marker variable of one pool (see [`Assign::pool_var`]).
    pub(crate) fn pool_wait_var(pool: &str) -> String {
        if pool == CANONICAL_POOL {
            CANDIDATES_WAIT_VAR.to_string()
        } else {
            format!("#pool_wait_{pool}")
        }
    }

    /// The names a pool key must satisfy: an identifier (`cc`, `my_name`),
    /// never reserved, and never engine-private (no leading `_`).
    fn validate_pool_name(node: &str, pool: &str) -> crate::Result<()> {
        let invalid = |detail: String| {
            crate::ActError::Action(format!(
                "act '{node}' declares an invalid assign pool: {detail}"
            ))
        };
        if RESERVED_POOL_NAMES.contains(&pool) {
            return Err(invalid(format!(
                "'{pool}' is a reserved name; pick another key for the pool"
            )));
        }
        let valid = pool
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic())
            && pool
                .chars()
                .all(|rest| rest.is_ascii_alphanumeric() || rest == '_');
        if !valid {
            return Err(invalid(format!(
                "'{pool}' is not a pool name: use letters, digits and underscores, starting with a letter"
            )));
        }
        Ok(())
    }

    /// The pool specs one act resolves at start — its own declarations and,
    /// for every key it does not declare itself, the nearest ancestor's
    /// declaration. The chain stops at a container: a container's pools are
    /// its own (its fan-out `in` and join read them), so its children inherit
    /// the computed result through the scope chain (`$assign.candidates.*`)
    /// and never re-ask the pool their container already asked. The own
    /// declarations are refused where they cannot mean a pool: a
    /// package-less node, the waiting subflow, or a `Func`/`Msg` leaf. An
    /// inherited pool rides the same placements except it skips silently on
    /// them — a self-completing or package-less child carries no pool.
    /// Pushed acts read their own declarations off the durable `#act` fact
    /// only.
    pub(crate) fn declared_pools(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        model_node: &crate::scheduler::core::model::model_graph::NodeId,
    ) -> crate::Result<Vec<CandidatesSpec>> {
        let Some(model) = crate::scheduler::shell::model_of(snapshot)? else {
            return Ok(Vec::new());
        };
        let Some(step) = crate::scheduler::shell::find_step_in_model(&model, model_node.as_str())
        else {
            // A pushed act is not in the immutable model: its declaration
            // rides the execution's durable `#act` fact.
            return Self::pushed_declared_pools(snapshot, task_id);
        };
        let node = task_id.node.as_str();
        let mut pools = Vec::new();
        let mut own = std::collections::BTreeSet::new();
        if let Some(assign) = step.assign.as_ref() {
            if let Some(candidates) = assign.candidates.as_ref() {
                own.insert(CANONICAL_POOL.to_string());
                pools.push(CandidatesSpec {
                    pool: CANONICAL_POOL.to_string(),
                    params: candidates.params.clone(),
                    uses: candidates.uses.clone(),
                });
            }
            for (name, candidates) in &assign.others {
                Self::validate_pool_name(node, name)?;
                own.insert(name.clone());
                pools.push(CandidatesSpec {
                    pool: name.clone(),
                    params: candidates.params.clone(),
                    uses: candidates.uses.clone(),
                });
            }
        }
        // Inheritance: a step that declares an assign gets every pool key it
        // does not declare itself from the nearest ancestor that declares
        // it, computed in this act's own scope at its start. A step without
        // an own assign declares nothing — it reads its ancestors' pools
        // through the scope chain. The chain stops at containers (see
        // `nearest_declared_pools`), which is what keeps a container's
        // fan-out children from re-asking the pool their container computed:
        // whatever assign the child declares, the container's declaration
        // never crosses into it.
        let own_pools = !own.is_empty();
        if step.assign.is_some()
            && !matches!(
                step.uses.as_deref().map(str::trim),
                Some(crate::scheduler::shell::model::BLOCK_PACKAGE)
                    | Some(crate::scheduler::shell::model::PARALLEL_PACKAGE)
                    | Some(crate::scheduler::shell::model::SEQUENCE_PACKAGE)
            )
        {
            for (name, candidates) in
                crate::scheduler::shell::nearest_declared_pools(&model, model_node.as_str())
            {
                if own.contains(&name) {
                    continue;
                }
                Self::validate_pool_name(node, &name)?;
                pools.push(CandidatesSpec {
                    pool: name,
                    params: candidates.params,
                    uses: candidates.uses,
                });
            }
        }
        if pools.is_empty() {
            return Ok(pools);
        }
        // Placement is judged on this declaring act's own package — the
        // spec's `uses` names the pool's computing package, which is never a
        // container and would misjudge an auto act. An inherited pool skips
        // silently where it cannot live; an own declaration is an error.
        let uses = step
            .uses
            .as_deref()
            .map(str::trim)
            .filter(|uses| !uses.is_empty());
        let Some(uses) = uses else {
            if own_pools {
                return Err(crate::ActError::Action(format!(
                    "act '{node}' declares an assign pool but has no package"
                )));
            }
            return Ok(Vec::new());
        };
        if uses == crate::scheduler::shell::model::SUBFLOW_PACKAGE {
            if !own_pools {
                return Ok(Vec::new());
            }
            return Err(crate::ActError::Action(format!(
                "act '{node}' uses '{uses}', which cannot declare candidates"
            )));
        }
        let container = matches!(
            uses,
            crate::scheduler::shell::model::BLOCK_PACKAGE
                | crate::scheduler::shell::model::PARALLEL_PACKAGE
                | crate::scheduler::shell::model::SEQUENCE_PACKAGE
        );
        if !container && crate::scheduler::shell::auto_decision(snapshot, model_node.as_str())? {
            if !own_pools {
                return Ok(Vec::new());
            }
            return Err(crate::ActError::Action(format!(
                "act '{node}' uses '{uses}', which cannot declare candidates (it never waits for an answer)"
            )));
        }
        Ok(pools)
    }

    /// The pool specs of a pushed act, refused on the same placements.
    fn pushed_declared_pools(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
    ) -> crate::Result<Vec<CandidatesSpec>> {
        let Some(act) = crate::scheduler::shell::action::pushed_act_of(snapshot, task_id)? else {
            return Ok(Vec::new());
        };
        let Some(assign) = &act.assign else {
            return Ok(Vec::new());
        };
        let node = task_id.node.as_str();
        let mut pools = Vec::with_capacity(assign.others.len() + 1);
        if let Some(candidates) = assign.candidates.as_ref() {
            pools.push(CandidatesSpec {
                pool: CANONICAL_POOL.to_string(),
                params: candidates.params.clone(),
                uses: candidates.uses.clone(),
            });
        }
        for (name, candidates) in &assign.others {
            Self::validate_pool_name(node, name)?;
            pools.push(CandidatesSpec {
                pool: name.clone(),
                params: candidates.params.clone(),
                uses: candidates.uses.clone(),
            });
        }
        if pools.is_empty() {
            return Ok(pools);
        }
        for spec in &pools {
            Self::reject_invalid_placement(snapshot, node, &spec.uses)?;
        }
        Ok(pools)
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

    /// Whether one pool of this execution is already computed.
    pub(crate) fn pool_computed(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        pool: &str,
    ) -> bool {
        crate::scheduler::shell::task_variable(snapshot, task_id, &Self::pool_var(pool)).is_some()
    }

    /// Whether this execution is already waiting for one pool.
    pub(crate) fn pool_pending(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
        pool: &str,
    ) -> bool {
        crate::scheduler::shell::task_variable(snapshot, task_id, &Self::pool_wait_var(pool))
            .is_some()
    }

    /// Push one pool's candidates request under the waiting act: a `Ready`
    /// child with its start operation, riding the spec's package (`uses`)
    /// with the request's params (the serialized expression). The start
    /// announces the request's `Created` like any act's, which is what the
    /// client answers.
    pub(crate) fn ask(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        plan: &mut crate::scheduler::core::model::transition_plan::TransitionPlan,
        parent: &crate::scheduler::core::model::task_graph::TaskId,
        pool: &str,
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
        // The request carries the pool it answers, so the settlement folds
        // the answer under that pool's own key.
        plan.variable_writes.push(VariableWrite {
            scope: ask.clone(),
            name: CANDIDATES_ASK_VAR.to_string(),
            value: serde_json::json!({ "pool": pool, "uses": uses }).to_string(),
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
    /// request's answer into the act's variables under the request's own key
    /// (`candidates` and the `#candidates` marker for the canonical pool,
    /// `<name>` and `#pool_<name>` for a named one) and re-enter its start.
    /// A skipped request defers: the re-entered start asks again. A failed
    /// request is not ours — the ordinary error walk carries it. Returns
    /// whether the settlement was ours to handle.
    pub(crate) fn settle(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        plan: &mut crate::scheduler::core::model::transition_plan::TransitionPlan,
        parent: &crate::scheduler::core::model::task_graph::TaskId,
        settled: &crate::scheduler::core::model::task_graph::TaskId,
    ) -> crate::Result<bool> {
        // Only an act that waits for one of its pools settles a candidates
        // request: every other parent/child pair is the ordinary walk's
        // business.
        let Some(asked) =
            crate::scheduler::shell::task_variable(snapshot, settled, CANDIDATES_ASK_VAR)
        else {
            return Ok(false);
        };
        // The pool the settled request answers, off its ask marker. A plain
        // string marker (a request pushed before named pools existed) answers
        // the canonical pool.
        let pool: String = match serde_json::from_str::<serde_json::Value>(asked) {
            Ok(serde_json::Value::Object(map)) => map
                .get("pool")
                .and_then(|pool| pool.as_str())
                .unwrap_or(CANONICAL_POOL)
                .to_string(),
            _ => CANONICAL_POOL.to_string(),
        };
        if !Self::pool_pending(snapshot, parent, &pool) {
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
                // "${{ @candidates }}"`, `owner: "${{ @candidates[0] }}"`);
                // `participants`/`count` ride along for the flows that want
                // code/name or a quorum number. The propagate's carry
                // already folds the request's own variables into the act's
                // scope; this writes the pool under its own key, skipping
                // duplicates so the plan stays valid.
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
                Self::fold_pool(plan, parent, &pool, computed);
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
    /// produced: fold it into the declaring act's variables under the pool's
    /// own key — the same shape a settled answer lands in — and re-enter its
    /// start, which finds the pool in scope and proceeds without a round
    /// trip.
    pub(crate) fn settle_local_pool(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        plan: &mut crate::scheduler::core::model::transition_plan::TransitionPlan,
        parent: &crate::scheduler::core::model::task_graph::TaskId,
        pool: &str,
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
        Self::fold_pool(plan, parent, pool, computed);
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

    /// Write one computed pool into the act's variables under its own key:
    /// the ids list (the bare `@candidates`/`@<name>` inheritable variable
    /// the fan-out and owner expressions read), the durable `#candidates`/
    /// `#pool_<name>` marker the re-entered start reads as "pool computed",
    /// and the wait marker cleared. The canonical pool also writes the shared
    /// `@participants`/`@count` answer fields, only when the plan does not
    /// already carry them. Every answer name is *inheritable* (`@`): the pool
    /// belongs to the declaring act and its children — it never folds into a
    /// parent scope when the act settles, so no later task inherits a stale
    /// pool from a step that ran before it.
    fn fold_pool(
        plan: &mut crate::scheduler::core::model::transition_plan::TransitionPlan,
        parent: &crate::scheduler::core::model::task_graph::TaskId,
        pool: &str,
        computed: serde_json::Value,
    ) {
        let ids = computed
            .get("ids")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let mut writes = vec![
            (
                format!("@{pool}"),
                serde_json::to_string(&ids).unwrap_or_default(),
            ),
            (Self::pool_var(pool), computed.to_string()),
            (Self::pool_wait_var(pool), "null".to_string()),
        ];
        if pool == CANONICAL_POOL {
            for name in ["participants", "count"] {
                let value = computed
                    .get(name)
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let carried = plan
                    .variable_writes
                    .iter()
                    .any(|write| write.scope == *parent && write.name == format!("@{name}"));
                if !carried {
                    writes.push((
                        format!("@{name}"),
                        serde_json::to_string(&value).unwrap_or_default(),
                    ));
                }
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
