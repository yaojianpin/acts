use super::*;
use crate::scheduler::core::commit::process::TaskSnapshot;
use crate::scheduler::core::model::operation::ClaimToken;
use crate::scheduler::core::model::transition_plan::ProcessVersion;
use std::collections::{BTreeMap, HashMap};

fn id(task: &str, generation: u64) -> TaskId {
    TaskId::new(task, generation).unwrap()
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
            node: task.into(),
            generation: 0,
            sequence,
        },
        claim: claim(),
    }
}
fn task(task: &str, generation: u64, state: TaskState) -> (TaskId, TaskSnapshot) {
    (
        id(task, generation),
        TaskSnapshot {
            model_node: node(task),
            parent: None,
            predecessors: vec![],
            state,
            revision: 2,
        },
    )
}
fn snapshot(tasks: Vec<(TaskId, TaskSnapshot)>) -> ProcessSnapshot {
    ProcessSnapshot {
        process_id: "p1".into(),
        version: ProcessVersion {
            graph: 1,
            process: 3,
            operations: 2,
        },
        tasks: HashMap::from_iter(tasks),
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
        node: task.into(),
        model_node: node(task),
        plan,
        predecessors,
        vars: Vec::new(),
        auto: true,
    }
}
fn fanout_command(
    source: &str,
    source_state: TaskState,
    children: Vec<ContainerChild>,
) -> (ProcessSnapshot, ContainerFanoutCommand) {
    let snapshot = snapshot(vec![
        task(source, 0, source_state),
        task("$", 0, TaskState::Completed),
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
        source_park: TaskState::Running,
    };
    (snapshot, command)
}

mod catch_join;
mod fanout;
mod flow;
mod join;
