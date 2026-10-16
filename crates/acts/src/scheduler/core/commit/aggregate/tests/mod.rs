use super::*;
use crate::scheduler::core::model::operation::ClaimToken;
use crate::scheduler::core::model::operation::OperationId;
use crate::scheduler::core::model::operation::OperationKind;
use crate::scheduler::core::model::transition_plan::OperationIntent;
use crate::scheduler::core::model::transition_plan::OutboxIntent;
use crate::scheduler::core::model::transition_plan::StateChange;
use crate::scheduler::core::model::transition_plan::VariableWrite;

fn id() -> TaskId {
    TaskId::new("t1", 0).unwrap()
}
fn repository() -> MemoryProcessRepository {
    let mut snapshot = ProcessSnapshot::empty("p1").unwrap();
    snapshot.tasks.insert(
        id(),
        TaskSnapshot {
            model_node: NodeId::parse("t1").unwrap(),
            parent: None,
            predecessors: vec![],
            state: TaskState::Running,
            revision: 2,
        },
    );
    let mut repo = MemoryProcessRepository::default();
    repo.insert(snapshot).unwrap();
    repo
}
fn plan() -> TransitionPlan {
    TransitionPlan {
        process_id: "p1".into(),
        expected: ProcessVersion {
            graph: 0,
            process: 0,
            operations: 0,
        },
        task_creates: Vec::new(),
        state_changes: vec![StateChange {
            task: id(),
            expected_revision: 2,
            from: TaskState::Running,
            to: TaskState::Completed,
        }],
        variable_writes: vec![VariableWrite {
            scope: id(),
            name: "result".into(),
            value: "42".into(),
        }],
        source_operation: None,
        operations: vec![OperationIntent {
            id: OperationId {
                process_id: "p1".into(),
                node: "t1".into(),
                generation: 0,
                sequence: 1,
            },
            kind: OperationKind::PropagateTask,
        }],
        outbox: vec![OutboxIntent {
            key: "complete:p1:t1:0".into(),
            topic: "task.completed".into(),
            payload: "{}".into(),
            receipt: None,
        }],
    }
}

mod advance;
mod canonical;
mod commit;
mod repository;
