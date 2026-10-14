//! The pure part of a timed-out execution's branch dispatch.
//!
//! The legacy engine fired a step's `timeouts` from the process tick: while the
//! step was `Running`/`Interrupt`, `Step::on_timeout` froze the task's cost into
//! its data (the `__cost` task datum `$cost()`/`$cost_in(...)` read) and
//! dispatched the first declared branch that had neither fired (the one-shot
//! `__timeouts` marker) nor was in flight. A branch whose guard did not hold
//! skipped *without* claiming, so a later tick re-evaluated it — that is what
//! opens the second window of a `timeouts` list.
//!
//! The kernel keeps the same three facts in the aggregate — the cost start
//! (`__started`), the frozen cost (`__cost`) and the one-shot marker
//! (`__timeouts`) — and this module is the pure part of the mechanism: which
//! branch the next tick may dispatch, what its cost is, and how the marker
//! advances. The host supplies `now` and the evaluated guard; nothing here
//! reads a clock, a spec or a store.

use crate::runtime::core::commit::aggregate::AggregateSnapshot;
use crate::runtime::core::model::execution_graph::ExecutionId;
use crate::runtime::core::model::model_graph::NodeId;
use crate::runtime::core::model::transition_plan::OperationCommit;
use crate::runtime::core::model::transition_plan::VariableWrite;
use crate::runtime::core::plan::container::ContainerChild;
use crate::runtime::core::plan::container::ContainerChildPlan;
use crate::runtime::core::plan::container::ContainerFanoutCommand;
use crate::runtime::core::plan::container::ContainerPredecessor;
use crate::utils::consts;

/// The cost start of one execution, written by its own start hop (`Ready →
/// Running`/`Waiting`). Not part of the expression variable space: it is a
/// kernel fact, so it stays `__`-private and never reaches a headless scope.
///
/// The legacy `Task` stamped its *creation* time and measured `cost()` from it.
/// The kernel pre-creates a run's chain executions at seed time, so creation
/// time would measure the whole run's age; the start hop is the closest durable
/// analogue — it is the moment the execution began its own work, which is what
/// the legacy stamp meant for a step its predecessor scheduled.
pub(crate) const STARTED_VAR: &str = "__started";

/// One declared timeout branch's durable admission facts: the model-supplied
/// parts (the guard itself is evaluated by the host, on the engine's own
/// environment).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TimeoutBranch {
    pub(crate) task_id: String,
    pub(crate) model_node: NodeId,
    /// The branch's declared model vars, seeded on its own scope.
    pub(crate) vars: Vec<(String, String)>,
    /// The import's auto decision for the branch (whether it self-completes).
    pub(crate) auto: bool,
}

/// Which declared branch the next dispatch round examines.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TimeoutCandidate {
    /// The i-th declared branch has not fired and has no live execution: this
    /// round evaluates its guard and dispatches it when the guard holds.
    Branch(usize),
    /// The first un-fired branch is in flight: the round stops here — exactly
    /// like the legacy dispatcher — so the branch declared after it waits for
    /// its turn instead of running ahead of it.
    Blocked,
    /// Every declared branch already fired.
    None,
}

/// The branches that already fired for one execution, from the one-shot marker
/// (the legacy `__timeouts` task datum, a JSON array).
pub(crate) fn claimed(snapshot: &AggregateSnapshot, owner: &ExecutionId) -> Vec<String> {
    marker(snapshot, owner).unwrap_or_default()
}

fn marker(snapshot: &AggregateSnapshot, owner: &ExecutionId) -> Option<Vec<String>> {
    let raw = snapshot
        .variables
        .get(&(owner.clone(), consts::TASK_TIMEOUTS.to_string()))?;
    serde_json::from_str(raw).ok()
}

/// The marker after `branch` fired: the one-shot claim of one branch,
/// appended to the ones already taken (legacy `Task::claim_timeout`).
pub(crate) fn claim(claimed: &[String], branch: &str) -> Vec<String> {
    let mut next = claimed.to_vec();
    if !next.iter().any(|id| id == branch) {
        next.push(branch.to_string());
    }
    next
}

/// The cost of one execution in milliseconds: what has passed since its start
/// hop, or zero when that stamp is missing (an aggregate seeded before the
/// timeout slice) or in the future (a clock that stepped backwards).
pub(crate) fn cost_ms(now_ms: i64, started: Option<&str>) -> i64 {
    let started = started
        .and_then(|raw| raw.parse::<i64>().ok())
        .unwrap_or(now_ms);
    now_ms.saturating_sub(started).max(0)
}

/// Whether one declared branch has a live (non-terminal) execution under
/// `owner` — the "in flight" half of the legacy dispatcher's check.
pub(crate) fn has_live_child(
    snapshot: &AggregateSnapshot,
    owner: &ExecutionId,
    branch: &str,
) -> bool {
    snapshot.executions.iter().any(|(id, execution)| {
        is_child_of(execution.parent.as_ref(), owner, id, branch) && !execution.state.is_terminal()
    })
}

fn is_child_of(
    parent: Option<&ExecutionId>,
    owner: &ExecutionId,
    id: &ExecutionId,
    branch: &str,
) -> bool {
    parent == Some(owner) && id.task_id == branch
}

/// Whether any of the declared timeout branches still has a live execution:
/// the owner may not complete before the branches it fired settle (the legacy
/// `auto_complete` counts every child task, timeout ones included).
pub(crate) fn has_live_children(
    snapshot: &AggregateSnapshot,
    owner: &ExecutionId,
    branches: &[TimeoutBranch],
) -> bool {
    branches
        .iter()
        .any(|branch| has_live_child(snapshot, owner, &branch.task_id))
}

/// Which branch the next round examines: declaration order, skipping the ones
/// that already fired, stopping at the first branch that is in flight.
pub(crate) fn candidate(
    snapshot: &AggregateSnapshot,
    owner: &ExecutionId,
    branches: &[TimeoutBranch],
) -> TimeoutCandidate {
    let fired = claimed(snapshot, owner);
    for (index, branch) in branches.iter().enumerate() {
        if fired.iter().any(|id| id == &branch.task_id) {
            continue;
        }
        if has_live_child(snapshot, owner, &branch.task_id) {
            return TimeoutCandidate::Blocked;
        }
        return TimeoutCandidate::Branch(index);
    }
    TimeoutCandidate::None
}

/// What one dispatch round admitted: the fan-out command that admits the due
/// branch (the container machinery's single-child shape, so a re-entered
/// container reuses the generation a rewind already recreated instead of
/// duplicating it) and the marker it left behind. Only the caller knows the
/// declared branches, so it — not this module — decides whether the timer
/// re-arms (see [`TimeoutDispatch::claimed_after`]).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TimeoutDispatch {
    pub(crate) command: ContainerFanoutCommand,
    /// The one-shot marker after this dispatch. The hop re-arms its timer
    /// while it is shorter than the declared branch list.
    pub(crate) claimed_after: Vec<String>,
}

/// The fan-out command for one due branch: the branch is admitted `Entry`
/// (Ready + `ExecuteTask`), the owner keeps its current state (a timed-out
/// execution runs or waits on, it does not park for a timeout), and the two
/// legacy task data of the owner ride the same atomic commit — the frozen cost
/// (`$cost()`'s window) and the advanced one-shot marker.
///
/// `extra_vars` are the branch's own scope writes on top of its declared vars
/// (the pinned failure of a guard that could not be evaluated — the legacy
/// branch task's `init` error).
pub(crate) fn dispatch(
    snapshot: &AggregateSnapshot,
    owner: &ExecutionId,
    source_operation: OperationCommit,
    branch: &TimeoutBranch,
    extra_vars: Vec<(String, String)>,
    claimed_after: &[String],
    cost_ms: i64,
) -> TimeoutDispatch {
    let mut vars = branch.vars.clone();
    vars.extend(extra_vars);
    TimeoutDispatch {
        command: ContainerFanoutCommand {
            process_id: snapshot.process_id.clone(),
            source: owner.clone(),
            source_operation,
            children: vec![ContainerChild {
                task_id: branch.task_id.clone(),
                model_node: branch.model_node.clone(),
                plan: ContainerChildPlan::Entry,
                predecessors: vec![ContainerPredecessor::Source],
                vars,
                auto: branch.auto,
            }],
            inputs: vec![
                (consts::TASK_COST.to_string(), cost_ms.to_string()),
                (
                    consts::TASK_TIMEOUTS.to_string(),
                    serde_json::to_string(claimed_after).expect("a list of strings is JSON"),
                ),
            ],
            // The owner runs (or waits) on: the legacy tick fires timeout
            // branches while the step is Running/Interrupt, it does not park
            // it. A state change here would need a matching reverse
            // transition, and none is meant.
            source_park: snapshot
                .executions
                .get(owner)
                .map(|execution| execution.state)
                .unwrap_or(crate::runtime::core::model::execution_graph::ExecutionState::Running),
        },
        claimed_after: claimed_after.to_vec(),
    }
}

/// The variable write that stamps an execution's cost start (see
/// [`STARTED_VAR`]).
pub(crate) fn start_write(owner: &ExecutionId, now_ms: i64) -> VariableWrite {
    VariableWrite {
        scope: owner.clone(),
        name: STARTED_VAR.to_string(),
        value: now_ms.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::core::commit::aggregate::ExecutionSnapshot;
    use crate::runtime::core::model::operation::OperationId;
    use crate::runtime::core::model::transition_plan::AggregateVersion;
    use crate::runtime::core::model::transition_plan::TransitionPlan;
    use std::collections::{BTreeMap, HashMap};

    fn id(task: &str, generation: u64) -> ExecutionId {
        ExecutionId::new(task, generation).unwrap()
    }

    fn node(name: &str) -> NodeId {
        NodeId::parse(name).unwrap()
    }

    fn commit(task: &str, sequence: u64) -> OperationCommit {
        OperationCommit {
            id: OperationId {
                process_id: "p1".into(),
                task_id: task.into(),
                generation: 0,
                sequence,
            },
            claim: crate::runtime::core::model::operation::ClaimToken {
                owner: "r1".into(),
                fence: 1,
                lease_until_ms: i64::MAX,
                heartbeat_revision: 0,
            },
        }
    }

    fn branch(task: &str) -> TimeoutBranch {
        TimeoutBranch {
            task_id: task.into(),
            model_node: node(task),
            vars: Vec::new(),
            auto: true,
        }
    }

    /// A snapshot with `root` (Running) and one owner execution `s1` (Waiting),
    /// plus whatever the test adds.
    fn snapshot(extra: Vec<(ExecutionId, ExecutionSnapshot)>) -> AggregateSnapshot {
        let root = id("$", 0);
        let owner = id("s1", 0);
        let mut executions = HashMap::from_iter([
            (
                root.clone(),
                ExecutionSnapshot {
                    model_node: node("$"),
                    parent: None,
                    predecessors: Vec::new(),
                    state: crate::runtime::core::model::execution_graph::ExecutionState::Running,
                    revision: 1,
                },
            ),
            (
                owner.clone(),
                ExecutionSnapshot {
                    model_node: node("s1"),
                    parent: Some(root),
                    predecessors: Vec::new(),
                    state: crate::runtime::core::model::execution_graph::ExecutionState::Waiting,
                    revision: 1,
                },
            ),
        ]);
        for (id, execution) in extra {
            executions.insert(id, execution);
        }
        AggregateSnapshot {
            process_id: "p1".into(),
            version: AggregateVersion {
                graph: 1,
                process: 2,
                operations: 2,
            },
            executions,
            variables: BTreeMap::new(),
            operations: HashMap::new(),
            outbox: HashMap::new(),
        }
    }

    fn child_of(
        owner: &ExecutionId,
        task: &str,
        state: crate::runtime::core::model::execution_graph::ExecutionState,
    ) -> (ExecutionId, ExecutionSnapshot) {
        (
            id(task, 0),
            ExecutionSnapshot {
                model_node: node(task),
                parent: Some(owner.clone()),
                predecessors: vec![owner.clone()],
                state,
                revision: 0,
            },
        )
    }

    #[test]
    fn the_candidate_is_the_first_branch_that_neither_fired_nor_is_in_flight() {
        let owner = id("s1", 0);
        let branches = vec![branch("t1"), branch("t2")];
        let mut snapshot = snapshot(Vec::new());
        assert_eq!(
            candidate(&snapshot, &owner, &branches),
            TimeoutCandidate::Branch(0)
        );
        // t1 fired: the round moves to t2.
        snapshot.variables.insert(
            (owner.clone(), consts::TASK_TIMEOUTS.to_string()),
            r#"["t1"]"#.to_string(),
        );
        assert_eq!(
            candidate(&snapshot, &owner, &branches),
            TimeoutCandidate::Branch(1)
        );
        // t2 fired too: nothing left.
        snapshot.variables.insert(
            (owner.clone(), consts::TASK_TIMEOUTS.to_string()),
            r#"["t1","t2"]"#.to_string(),
        );
        assert_eq!(
            candidate(&snapshot, &owner, &branches),
            TimeoutCandidate::None
        );
    }

    #[test]
    fn an_in_flight_branch_stops_the_round_instead_of_moving_on() {
        let owner = id("s1", 0);
        let branches = vec![branch("t1"), branch("t2")];
        // t1 in flight (unclaimed — a raced replay or a crash cut): the round
        // must not run ahead to t2.
        let in_flight = snapshot(vec![child_of(
            &owner,
            "t1",
            crate::runtime::core::model::execution_graph::ExecutionState::Running,
        )]);
        assert_eq!(
            candidate(&in_flight, &owner, &branches),
            TimeoutCandidate::Blocked
        );
        // A terminal t1 does not block; it only leaves the candidate at t2 when
        // its marker was taken, and at t1 itself when it was not.
        let settled = snapshot(vec![child_of(
            &owner,
            "t1",
            crate::runtime::core::model::execution_graph::ExecutionState::Completed,
        )]);
        assert_eq!(
            candidate(&settled, &owner, &branches),
            TimeoutCandidate::Branch(0)
        );
    }

    #[test]
    fn the_cost_counts_from_the_start_stamp_and_never_goes_negative() {
        assert_eq!(cost_ms(1_500, Some("1000")), 500);
        // A missing stamp (an aggregate seeded before this fact) counts as
        // "started now", so a guard cannot fire on an unmeasurable age.
        assert_eq!(cost_ms(1_500, None), 0);
        // A stamp in the future (a clock that stepped back) is not an age.
        assert_eq!(cost_ms(1_000, Some("1500")), 0);
        assert_eq!(cost_ms(1_500, Some("not a number")), 0);
    }

    #[test]
    fn the_claim_appends_one_branch_and_never_repeats_it() {
        assert_eq!(claim(&[], "t1"), vec!["t1".to_string()]);
        assert_eq!(
            claim(&["t1".to_string()], "t2"),
            vec!["t1".to_string(), "t2".to_string()]
        );
        assert_eq!(claim(&["t1".to_string()], "t1"), vec!["t1".to_string()]);
        // The marker is the legacy task datum, not a new key.
        assert_eq!(consts::TASK_TIMEOUTS, "__timeouts");
        assert_eq!(consts::TASK_COST, "__cost");
    }

    #[test]
    fn only_the_declared_branches_keep_the_owner_from_completing() {
        let owner = id("s1", 0);
        let branches = vec![branch("t1")];
        assert!(!has_live_children(&snapshot(Vec::new()), &owner, &branches));
        let live = snapshot(vec![child_of(
            &owner,
            "t1",
            crate::runtime::core::model::execution_graph::ExecutionState::Ready,
        )]);
        assert!(has_live_children(&live, &owner, &branches));
        // A settled branch does not hold the owner back.
        let settled = snapshot(vec![child_of(
            &owner,
            "t1",
            crate::runtime::core::model::execution_graph::ExecutionState::Skipped,
        )]);
        assert!(!has_live_children(&settled, &owner, &branches));
        // A child the model never declared as a timeout branch (a branch of a
        // branched step) is not this fact's business.
        let other = snapshot(vec![child_of(
            &owner,
            "b1",
            crate::runtime::core::model::execution_graph::ExecutionState::Running,
        )]);
        assert!(!has_live_children(&other, &owner, &branches));
    }

    #[test]
    fn the_dispatch_admits_one_entry_child_and_freezes_the_cost_and_marker() {
        let owner = id("s1", 0);
        let snapshot = snapshot(Vec::new());
        let dispatch = dispatch(
            &snapshot,
            &owner,
            commit("s1", 3),
            &branch("t1"),
            vec![(
                "__branch_error".into(),
                r#"{"code":"","message":"x"}"#.into(),
            )],
            &["t1".to_string()],
            1_617,
        );
        let command = &dispatch.command;
        assert_eq!(command.source, owner);
        assert_eq!(command.children.len(), 1);
        let child = &command.children[0];
        assert_eq!(child.task_id, "t1");
        assert_eq!(child.plan, ContainerChildPlan::Entry);
        assert_eq!(child.predecessors, vec![ContainerPredecessor::Source]);
        assert!(child.auto);
        assert_eq!(
            child.vars,
            vec![(
                "__branch_error".to_string(),
                r#"{"code":"","message":"x"}"#.to_string()
            )]
        );
        assert_eq!(
            command.inputs,
            vec![
                ("__cost".to_string(), "1617".to_string()),
                ("__timeouts".to_string(), r#"["t1"]"#.to_string()),
            ]
        );
        // The owner keeps the state it had: a timed-out execution is not parked
        // by a dispatch.
        assert_eq!(
            command.source_park,
            crate::runtime::core::model::execution_graph::ExecutionState::Waiting
        );
        // The plan the container machinery builds from it is legal.
        let plan = crate::runtime::core::plan::container::ContainerPlanner::plan_fanout(
            &snapshot, command,
        )
        .unwrap();
        plan.validate().unwrap();
        assert_eq!(plan.execution_creates.len(), 1);
        assert_eq!(
            plan.state_changes,
            Vec::new(),
            "a dispatch never moves the owner"
        );
    }

    #[test]
    fn the_dispatch_reports_the_marker_it_leaves_behind() {
        let owner = id("s1", 0);
        let snapshot = snapshot(Vec::new());
        // The caller re-arms its timer while the marker is shorter than the
        // declaration: one branch claimed of two declared re-arms, the last one
        // retires the timer.
        let one = dispatch(
            &snapshot,
            &owner,
            commit("s1", 3),
            &branch("t1"),
            Vec::new(),
            &["t1".to_string()],
            10,
        );
        assert_eq!(one.claimed_after, vec!["t1".to_string()]);
        assert!(one.claimed_after.len() < 2);
        let both = dispatch(
            &snapshot,
            &owner,
            commit("s1", 4),
            &branch("t2"),
            Vec::new(),
            &["t1".to_string(), "t2".to_string()],
            10,
        );
        assert!(!(both.claimed_after.len() < 2));
    }

    #[test]
    fn the_start_write_stamps_the_executions_own_scope() {
        let write = start_write(&id("s1", 0), 42);
        assert_eq!(write.name, "__started");
        assert_eq!(write.value, "42");
        assert_eq!(write.scope, id("s1", 0));
    }

    #[test]
    fn the_dispatch_command_plans_through_the_container_machinery() {
        // Guard against the dispatch drifting away from the container plan
        // shape: the command is a `ContainerFanoutCommand`, the plan a
        // `TransitionPlan` built by the same planner every other fan-out uses.
        let snapshot = snapshot(Vec::new());
        let command: ContainerFanoutCommand = dispatch(
            &snapshot,
            &id("s1", 0),
            commit("s1", 1),
            &branch("t1"),
            Vec::new(),
            &["t1".to_string()],
            0,
        )
        .command;
        let plan: TransitionPlan =
            crate::runtime::core::plan::container::ContainerPlanner::plan_fanout(
                &snapshot, &command,
            )
            .unwrap();
        assert_eq!(plan.process_id, "p1");
    }
}
