use super::*;

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
                .unwrap_or(crate::scheduler::core::model::execution_graph::ExecutionState::Running),
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
