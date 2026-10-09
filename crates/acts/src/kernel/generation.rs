use super::{
    AggregateSnapshot, ExecutionCreate, ExecutionId, ExecutionState, NodeId, OperationCommit,
    OperationId, OperationIntent, OperationKind, OutboxIntent, TransitionPlan,
};
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReenterCommand {
    pub(crate) process_id: String,
    /// The terminal execution requesting re-entry, normally a Backed task.
    pub(crate) source: ExecutionId,
    /// Stable logical task identity to instantiate again.
    pub(crate) target_task_id: String,
    pub(crate) source_operation: OperationCommit,
}

pub(crate) struct GenerationPlanner;

impl GenerationPlanner {
    pub(crate) fn plan_reentry(
        snapshot: &AggregateSnapshot,
        command: &ReenterCommand,
    ) -> Result<TransitionPlan, GenerationError> {
        if snapshot.process_id != command.process_id {
            return Err(GenerationError::WrongAggregate);
        }
        let source = snapshot
            .executions
            .get(&command.source)
            .ok_or_else(|| GenerationError::ExecutionNotFound(command.source.clone()))?;
        if source.state != ExecutionState::Backed {
            return Err(GenerationError::SourceNotBacked {
                source: command.source.clone(),
                state: source.state,
            });
        }
        let operation = &command.source_operation.id;
        if operation.process_id != command.process_id
            || operation.task_id != command.source.task_id
            || operation.generation != command.source.generation
        {
            return Err(GenerationError::SourceOperationTargetsDifferentExecution);
        }
        if command.target_task_id.trim().is_empty() {
            return Err(GenerationError::EmptyTargetTaskId);
        }
        let latest = snapshot
            .executions
            .iter()
            .filter(|(id, _)| id.task_id == command.target_task_id)
            .max_by_key(|(id, _)| id.generation)
            .ok_or_else(|| {
                GenerationError::TargetNeverInstantiated(command.target_task_id.clone())
            })?;
        let generation = latest
            .0
            .generation
            .checked_add(1)
            .ok_or(GenerationError::GenerationOverflow)?;
        let next = ExecutionId::new(command.target_task_id.clone(), generation)
            .map_err(|_| GenerationError::EmptyTargetTaskId)?;
        let next_operation = OperationId {
            process_id: command.process_id.clone(),
            task_id: next.task_id.clone(),
            generation: next.generation,
            sequence: 0,
        };
        let plan = TransitionPlan {
            process_id: command.process_id.clone(),
            expected: snapshot.version.clone(),
            execution_creates: vec![ExecutionCreate {
                execution: next.clone(),
                model_node: latest.1.model_node.clone(),
                parent: latest.1.parent.clone(),
                predecessors: vec![command.source.clone()],
                initial_state: ExecutionState::Ready,
            }],
            state_changes: Vec::new(),
            variable_writes: Vec::new(),
            source_operation: Some(command.source_operation.clone()),
            operations: vec![OperationIntent {
                id: next_operation,
                kind: OperationKind::ExecuteTask,
            }],
            outbox: vec![OutboxIntent {
                key: format!(
                    "reenter:{}:{}:{}:{}",
                    command.process_id,
                    next.task_id,
                    next.generation,
                    command.source_operation.id.sequence,
                ),
                topic: "task.reentered".into(),
                payload: format!("{}:{}", next.task_id, next.generation),
            }],
        };
        plan.validate()
            .map_err(|error| GenerationError::InvalidPlan(error.to_string()))?;
        Ok(plan)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum GenerationError {
    WrongAggregate,
    ExecutionNotFound(ExecutionId),
    SourceNotBacked {
        source: ExecutionId,
        state: ExecutionState,
    },
    SourceOperationTargetsDifferentExecution,
    EmptyTargetTaskId,
    TargetNeverInstantiated(String),
    GenerationOverflow,
    InvalidPlan(String),
}
impl fmt::Display for GenerationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "generation error: {self:?}")
    }
}
impl std::error::Error for GenerationError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::{AggregateVersion, ClaimToken, ExecutionSnapshot};
    use std::collections::{BTreeMap, HashMap};

    fn id(task: &str, generation: u64) -> ExecutionId {
        ExecutionId::new(task, generation).unwrap()
    }
    fn snapshot() -> AggregateSnapshot {
        AggregateSnapshot {
            process_id: "p1".into(),
            version: AggregateVersion {
                graph: 3,
                process: 8,
                operations: 6,
            },
            executions: HashMap::from([
                (
                    id("source", 1),
                    ExecutionSnapshot {
                        model_node: NodeId::parse("source").unwrap(),
                        parent: None,
                        predecessors: vec![],
                        state: ExecutionState::Backed,
                        revision: 4,
                    },
                ),
                (
                    id("target", 0),
                    ExecutionSnapshot {
                        model_node: NodeId::parse("target-node").unwrap(),
                        parent: None,
                        predecessors: vec![],
                        state: ExecutionState::Completed,
                        revision: 3,
                    },
                ),
                (
                    id("target", 1),
                    ExecutionSnapshot {
                        model_node: NodeId::parse("target-node").unwrap(),
                        parent: None,
                        predecessors: vec![id("source", 0)],
                        state: ExecutionState::Completed,
                        revision: 3,
                    },
                ),
            ]),
            variables: BTreeMap::new(),
            operations: HashMap::new(),
            outbox: HashMap::new(),
        }
    }
    fn command() -> ReenterCommand {
        ReenterCommand {
            process_id: "p1".into(),
            source: id("source", 1),
            target_task_id: "target".into(),
            source_operation: OperationCommit {
                id: OperationId {
                    process_id: "p1".into(),
                    task_id: "source".into(),
                    generation: 1,
                    sequence: 8,
                },
                claim: ClaimToken {
                    owner: "runtime-a".into(),
                    fence: 7,
                    lease_until_ms: i64::MAX,
                    heartbeat_revision: 0,
                },
            },
        }
    }

    #[test]
    fn reentry_creates_new_generation_and_never_mutates_history() {
        let plan = GenerationPlanner::plan_reentry(&snapshot(), &command()).unwrap();
        assert!(plan.state_changes.is_empty());
        assert_eq!(plan.execution_creates[0].execution, id("target", 2));
        assert_eq!(
            plan.execution_creates[0].initial_state,
            ExecutionState::Ready
        );
        assert_eq!(plan.operations[0].id.generation, 2);
    }

    #[test]
    fn non_backed_source_cannot_reenter() {
        let mut snapshot = snapshot();
        snapshot.executions.get_mut(&id("source", 1)).unwrap().state = ExecutionState::Waiting;
        assert!(matches!(
            GenerationPlanner::plan_reentry(&snapshot, &command()),
            Err(GenerationError::SourceNotBacked { .. })
        ));
    }
}
