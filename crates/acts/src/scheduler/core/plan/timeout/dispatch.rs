use super::*;

use crate::scheduler::core::plan::ActPlan;

/// What one dispatch round admitted: the fan-out command that admits the due
/// branch (the container machinery's single-child shape, so a re-entered
/// container reuses the generation a rewind already recreated instead of
/// duplicating it) and the marker it left behind. Only the caller knows the
/// declared branches, so it — not this module — decides whether the timer
/// re-arms (see [`TimeoutDispatch::claimed_after`]).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TimeoutDispatch {
    pub(crate) command: ContainerFanoutCommand,
    /// The one-shot marker after this dispatch. The operation re-arms its timer
    /// while it is shorter than the declared branch list.
    pub(crate) claimed_after: Vec<String>,
}

/// The command of one dispatch round: whose timer fired, on which operation
/// claim, for which declared branch.
pub(crate) struct DispatchCommand<'a> {
    /// The execution whose timer fired.
    pub(crate) owner: &'a TaskId,
    /// The operation the timeout operation claimed the timer with.
    pub(crate) source_operation: OperationCommit,
    /// The branch that is due.
    pub(crate) branch: &'a TimeoutBranch,
    /// The branch's own scope writes on top of its declared vars (the pinned
    /// failure of a guard that could not be evaluated — the legacy branch
    /// task's `init` error).
    pub(crate) extra_vars: Vec<(String, String)>,
    /// The one-shot marker this dispatch advances.
    pub(crate) claimed_after: &'a [String],
    /// The owner's frozen cost in milliseconds.
    pub(crate) cost_ms: i64,
}

/// The plan of one dispatch round: which fan-out command admits a due timeout
/// branch, given the branch, the claim and the owner's frozen cost.
pub(crate) struct Dispatch;

impl ActPlan for Dispatch {
    type Reading<'a> = &'a ProcessSnapshot;
    type Command<'a> = DispatchCommand<'a>;
    type Output = TimeoutDispatch;
    type Error = std::convert::Infallible;

    /// The fan-out command for one due branch: the branch is admitted `Entry`
    /// (Ready + `ExecuteTask`), the owner keeps its current state (a timed-out
    /// execution runs or waits on, it does not park for a timeout), and the two
    /// legacy task data of the owner ride the same atomic commit — the frozen cost
    /// (`$cost()`'s window) and the advanced one-shot marker.
    ///
    /// `extra_vars` are the branch's own scope writes on top of its declared vars
    /// (the pinned failure of a guard that could not be evaluated — the legacy
    /// branch task's `init` error).
    fn plan(
        reading: Self::Reading<'_>,
        command: &Self::Command<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let snapshot = reading;
        let owner = command.owner;
        let source_operation = command.source_operation.clone();
        let branch = command.branch;
        let extra_vars = command.extra_vars.clone();
        let claimed_after = command.claimed_after;
        let cost_ms = command.cost_ms;
        let mut vars = branch.vars.clone();
        vars.extend(extra_vars);
        Ok(TimeoutDispatch {
            command: ContainerFanoutCommand {
                process_id: snapshot.process_id.clone(),
                source: owner.clone(),
                source_operation,
                children: vec![ContainerChild {
                    node: branch.node.clone(),
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
                    .tasks
                    .get(owner)
                    .map(|task| task.state)
                    .unwrap_or(crate::scheduler::core::model::task_graph::TaskState::Running),
            },
            claimed_after: claimed_after.to_vec(),
        })
    }
}

/// The variable write that stamps an execution's cost start (see
/// [`STARTED_VAR`]).
pub(crate) fn start_write(owner: &TaskId, now_ms: i64) -> VariableWrite {
    VariableWrite {
        scope: owner.clone(),
        name: STARTED_VAR.to_string(),
        value: now_ms.to_string(),
    }
}
