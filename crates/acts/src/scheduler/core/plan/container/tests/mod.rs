use super::*;
use crate::scheduler::core::commit::aggregate::ExecutionSnapshot;
use crate::scheduler::core::model::operation::ClaimToken;
use crate::scheduler::core::model::transition_plan::AggregateVersion;
use std::collections::{BTreeMap, HashMap};

fn id(task: &str, generation: u64) -> ExecutionId {
    ExecutionId::new(task, generation).unwrap()
}
fn node(name: &str) -> NodeId {
    NodeId::parse(name).unwrap()
}
fn claim() -> ClaimToken {
    ClaimToken {
        owner: "r1".into(),
        fence: 2,
        lease_until_ms: i64::MAX,
        heartbeat_revision: 0,
    }
}
fn commit(task: &str, sequence: u64) -> OperationCommit {
    OperationCommit {
        id: OperationId {
            process_id: "p1".into(),
            task_id: task.into(),
            generation: 0,
            sequence,
        },
        claim: claim(),
    }
}
fn execution(
    task: &str,
    generation: u64,
    state: ExecutionState,
) -> (ExecutionId, ExecutionSnapshot) {
    (
        id(task, generation),
        ExecutionSnapshot {
            model_node: node(task),
            parent: None,
            predecessors: vec![],
            state,
            revision: 2,
        },
    )
}
fn snapshot(executions: Vec<(ExecutionId, ExecutionSnapshot)>) -> AggregateSnapshot {
    AggregateSnapshot {
        process_id: "p1".into(),
        version: AggregateVersion {
            graph: 1,
            process: 3,
            operations: 2,
        },
        executions: HashMap::from_iter(executions),
        variables: BTreeMap::new(),
        operations: HashMap::new(),
        outbox: HashMap::new(),
    }
}
fn child(
    task: &str,
    plan: ContainerChildPlan,
    predecessors: Vec<ContainerPredecessor>,
) -> ContainerChild {
    ContainerChild {
        task_id: task.into(),
        model_node: node(task),
        plan,
        predecessors,
        vars: Vec::new(),
        auto: true,
    }
}
fn fanout_command(
    source: &str,
    source_state: ExecutionState,
    children: Vec<ContainerChild>,
) -> (AggregateSnapshot, ContainerFanoutCommand) {
    let snapshot = snapshot(vec![
        execution(source, 0, source_state),
        execution("$", 0, ExecutionState::Completed),
    ]);
    let command = ContainerFanoutCommand {
        process_id: "p1".into(),
        source: id(source, 0),
        source_operation: commit(source, 4),
        children,
        inputs: Vec::new(),
        // Every container parks `Running` while its children run (the
        // legacy step's state); only a failed step's catch fan-out parks
        // elsewhere, and that test overrides this.
        source_park: ExecutionState::Running,
    };
    (snapshot, command)
}

mod catch_join;
mod fanout;
mod flow;
mod join;
