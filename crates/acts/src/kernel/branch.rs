use super::{
    AggregateSnapshot, ExecutionCreate, ExecutionId, ExecutionState, NodeId, OperationCommit,
    OperationId, OperationIntent, OperationKind, OutboxIntent, TransitionPlan,
};
use std::collections::HashSet;
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PredicateValue {
    True,
    False,
    Null,
    Invalid { message: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BranchCandidate {
    pub(crate) task_id: String,
    pub(crate) model_node: NodeId,
    pub(crate) predicate: PredicateValue,
    pub(crate) is_else: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum BranchDecision {
    Selected(Vec<BranchCandidate>),
    NoMatch,
    Waiting { dependencies: Vec<String> },
    Invalid { message: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PredicateBasis {
    pub(crate) graph: u64,
    pub(crate) process: u64,
}

impl PredicateBasis {
    pub(crate) fn from_snapshot(snapshot: &AggregateSnapshot) -> Self {
        Self {
            graph: snapshot.version.graph,
            process: snapshot.version.process,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BranchCommand {
    pub(crate) process_id: String,
    pub(crate) branch: ExecutionId,
    pub(crate) candidates: Vec<BranchCandidate>,
    /// Aggregate version against which every predicate value was evaluated.
    pub(crate) predicate_basis: PredicateBasis,
    pub(crate) unresolved_dependencies: Vec<String>,
    pub(crate) source_operation: OperationCommit,
}

/// A branch decision is either ready to commit or explicitly deferred.
/// Deferral is not an error and must never be converted into quarantine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum BranchPlan {
    Ready(Box<TransitionPlan>),
    Deferred { dependencies: Vec<String> },
}

pub(crate) struct BranchPlanner;

impl BranchPlanner {
    /// Validate the command's structural facts before considering dependency
    /// deferral. An invalid model must never masquerade as healthy waiting.
    pub(crate) fn validate_command(command: &BranchCommand) -> Result<(), BranchError> {
        let else_count = command
            .candidates
            .iter()
            .filter(|item| item.is_else)
            .count();
        if else_count > 1 {
            return Err(BranchError::InvalidDecision(
                "multiple else branches".into(),
            ));
        }
        for candidate in &command.candidates {
            if candidate.task_id.trim().is_empty() {
                return Err(BranchError::InvalidTargetTaskId(candidate.task_id.clone()));
            }
            if candidate.is_else {
                continue;
            }
            match &candidate.predicate {
                PredicateValue::True | PredicateValue::False => {}
                PredicateValue::Null => {
                    return Err(BranchError::InvalidDecision(format!(
                        "branch '{}' predicate evaluated to null",
                        candidate.task_id
                    )));
                }
                PredicateValue::Invalid { message } => {
                    return Err(BranchError::InvalidDecision(message.clone()));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn decide(command: &BranchCommand) -> BranchDecision {
        if let Err(error) = Self::validate_command(command) {
            return BranchDecision::Invalid {
                message: error.to_string(),
            };
        }
        if !command.unresolved_dependencies.is_empty() {
            let mut dependencies = command.unresolved_dependencies.clone();
            dependencies.sort();
            dependencies.dedup();
            return BranchDecision::Waiting { dependencies };
        }
        let mut selected = Vec::new();
        let mut fallback = None;
        for candidate in &command.candidates {
            if candidate.is_else {
                fallback = Some(candidate.clone());
            } else if matches!(candidate.predicate, PredicateValue::True) {
                selected.push(candidate.clone());
            }
        }
        if selected.is_empty()
            && let Some(fallback) = fallback
        {
            selected.push(fallback);
        }
        if selected.is_empty() {
            BranchDecision::NoMatch
        } else {
            BranchDecision::Selected(selected)
        }
    }

    pub(crate) fn plan(
        snapshot: &AggregateSnapshot,
        command: &BranchCommand,
    ) -> Result<BranchPlan, BranchError> {
        Self::validate_command(command)?;
        validate_envelope(snapshot, command)?;
        let authoritative_basis = PredicateBasis::from_snapshot(snapshot);
        if command.predicate_basis != authoritative_basis {
            return Err(BranchError::StalePredicateBasis {
                evaluated: command.predicate_basis.clone(),
                authoritative: authoritative_basis,
            });
        }
        let branch = snapshot
            .executions
            .get(&command.branch)
            .ok_or_else(|| BranchError::ExecutionNotFound(command.branch.clone()))?;
        if branch.state != ExecutionState::Running {
            return Err(BranchError::BranchNotRunning(branch.state));
        }
        let decision = Self::decide(command);
        match decision {
            BranchDecision::Waiting { dependencies } => Ok(BranchPlan::Deferred { dependencies }),
            BranchDecision::Invalid { message } => Err(BranchError::InvalidDecision(message)),
            BranchDecision::NoMatch => validated(TransitionPlan {
                process_id: command.process_id.clone(),
                expected: snapshot.version.clone(),
                execution_creates: Vec::new(),
                state_changes: vec![super::StateChange {
                    execution: command.branch.clone(),
                    expected_revision: branch.revision,
                    from: ExecutionState::Running,
                    to: ExecutionState::Completed,
                }],
                variable_writes: Vec::new(),
                source_operation: Some(command.source_operation.clone()),
                operations: vec![next_propagation(command, command.branch.clone())?],
                outbox: vec![branch_outbox(command, "no_match")],
            }),
            BranchDecision::Selected(candidates) => {
                let mut task_ids = HashSet::new();
                let mut creates = Vec::with_capacity(candidates.len());
                let mut operations = Vec::with_capacity(candidates.len());
                for candidate in candidates {
                    if !task_ids.insert(candidate.task_id.clone()) {
                        return Err(BranchError::DuplicateSelection(candidate.task_id));
                    }
                    let generation = next_generation(snapshot, &candidate.task_id)?;
                    let execution = ExecutionId::new(candidate.task_id.clone(), generation)
                        .map_err(|_| BranchError::InvalidExecutionId(candidate.task_id.clone()))?;
                    creates.push(ExecutionCreate {
                        execution: execution.clone(),
                        model_node: candidate.model_node,
                        parent: Some(command.branch.clone()),
                        predecessors: vec![command.branch.clone()],
                        initial_state: ExecutionState::Ready,
                    });
                    operations.push(OperationIntent {
                        id: OperationId {
                            process_id: command.process_id.clone(),
                            task_id: execution.task_id.clone(),
                            generation,
                            sequence: 0,
                        },
                        kind: OperationKind::ExecuteTask,
                    });
                }
                let plan = TransitionPlan {
                    process_id: command.process_id.clone(),
                    expected: snapshot.version.clone(),
                    execution_creates: creates,
                    state_changes: vec![super::StateChange {
                        execution: command.branch.clone(),
                        expected_revision: branch.revision,
                        from: ExecutionState::Running,
                        to: ExecutionState::Waiting,
                    }],
                    variable_writes: Vec::new(),
                    source_operation: Some(command.source_operation.clone()),
                    operations,
                    outbox: vec![branch_outbox(command, "selected")],
                };
                validated(plan)
            }
        }
    }
}

fn validated(plan: TransitionPlan) -> Result<BranchPlan, BranchError> {
    plan.validate()
        .map_err(|error| BranchError::InvalidPlan(error.to_string()))?;
    Ok(BranchPlan::Ready(Box::new(plan)))
}

fn validate_envelope(
    snapshot: &AggregateSnapshot,
    command: &BranchCommand,
) -> Result<(), BranchError> {
    if snapshot.process_id != command.process_id {
        return Err(BranchError::WrongAggregate);
    }
    let source = &command.source_operation.id;
    if source.process_id != command.process_id
        || source.task_id != command.branch.task_id
        || source.generation != command.branch.generation
    {
        return Err(BranchError::SourceOperationTargetsDifferentExecution);
    }
    Ok(())
}

fn next_generation(snapshot: &AggregateSnapshot, task_id: &str) -> Result<u64, BranchError> {
    snapshot
        .executions
        .keys()
        .filter(|id| id.task_id == task_id)
        .map(|id| id.generation)
        .max()
        .map_or(Ok(0), |value| {
            value.checked_add(1).ok_or(BranchError::GenerationOverflow)
        })
}

fn next_propagation(
    command: &BranchCommand,
    execution: ExecutionId,
) -> Result<OperationIntent, BranchError> {
    let sequence = command
        .source_operation
        .id
        .sequence
        .checked_add(1)
        .ok_or(BranchError::SequenceOverflow)?;
    Ok(OperationIntent {
        id: OperationId {
            process_id: command.process_id.clone(),
            task_id: execution.task_id,
            generation: execution.generation,
            sequence,
        },
        kind: OperationKind::PropagateTask,
    })
}

fn branch_outbox(command: &BranchCommand, outcome: &str) -> OutboxIntent {
    OutboxIntent {
        key: format!(
            "branch:{}:{}:{}:{}",
            command.process_id,
            command.branch.task_id,
            command.branch.generation,
            command.source_operation.id.sequence
        ),
        topic: format!("branch.{outcome}"),
        payload: format!("{}:{}", command.branch.task_id, command.branch.generation),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum BranchError {
    WrongAggregate,
    SourceOperationTargetsDifferentExecution,
    ExecutionNotFound(ExecutionId),
    BranchNotRunning(ExecutionState),
    InvalidDecision(String),
    InvalidTargetTaskId(String),
    InvalidExecutionId(String),
    StalePredicateBasis {
        evaluated: PredicateBasis,
        authoritative: PredicateBasis,
    },
    DuplicateSelection(String),
    GenerationOverflow,
    SequenceOverflow,
    InvalidPlan(String),
}
impl fmt::Display for BranchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "branch error: {self:?}")
    }
}
impl std::error::Error for BranchError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::{AggregateVersion, ClaimToken, ExecutionSnapshot};
    use std::collections::{BTreeMap, HashMap};

    fn id(name: &str, generation: u64) -> ExecutionId {
        ExecutionId::new(name, generation).unwrap()
    }
    fn ready(result: Result<BranchPlan, BranchError>) -> TransitionPlan {
        match result.unwrap() {
            BranchPlan::Ready(plan) => *plan,
            BranchPlan::Deferred { dependencies } => {
                panic!("expected ready plan, deferred on {dependencies:?}")
            }
        }
    }

    fn snapshot() -> AggregateSnapshot {
        AggregateSnapshot {
            process_id: "p1".into(),
            version: AggregateVersion {
                graph: 1,
                process: 3,
                operations: 2,
            },
            executions: HashMap::from([(
                id("branch", 0),
                ExecutionSnapshot {
                    model_node: NodeId::parse("branch").unwrap(),
                    parent: None,
                    predecessors: vec![],
                    state: ExecutionState::Running,
                    revision: 2,
                },
            )]),
            variables: BTreeMap::new(),
            operations: HashMap::new(),
            outbox: HashMap::new(),
        }
    }
    fn candidate(name: &str, predicate: PredicateValue, is_else: bool) -> BranchCandidate {
        BranchCandidate {
            task_id: name.into(),
            model_node: NodeId::parse(name).unwrap(),
            predicate,
            is_else,
        }
    }
    fn command(candidates: Vec<BranchCandidate>) -> BranchCommand {
        BranchCommand {
            process_id: "p1".into(),
            branch: id("branch", 0),
            candidates,
            predicate_basis: PredicateBasis {
                graph: 1,
                process: 3,
            },
            unresolved_dependencies: Vec::new(),
            source_operation: OperationCommit {
                id: OperationId {
                    process_id: "p1".into(),
                    task_id: "branch".into(),
                    generation: 0,
                    sequence: 4,
                },
                claim: ClaimToken {
                    owner: "r1".into(),
                    fence: 2,
                    lease_until_ms: i64::MAX,
                    heartbeat_revision: 0,
                },
            },
        }
    }

    #[test]
    fn no_match_is_explicit_and_completes_branch() {
        let plan = ready(BranchPlanner::plan(
            &snapshot(),
            &command(vec![candidate("a", PredicateValue::False, false)]),
        ));
        assert!(plan.execution_creates.is_empty());
        assert_eq!(plan.state_changes[0].to, ExecutionState::Completed);
        assert_eq!(plan.operations[0].kind, OperationKind::PropagateTask);
    }

    #[test]
    fn else_is_selected_only_when_no_condition_matches() {
        let plan = ready(BranchPlanner::plan(
            &snapshot(),
            &command(vec![
                candidate("a", PredicateValue::False, false),
                candidate("else", PredicateValue::True, true),
            ]),
        ));
        assert_eq!(plan.execution_creates[0].execution, id("else", 0));
        assert_eq!(plan.state_changes[0].to, ExecutionState::Waiting);
    }

    #[test]
    fn null_is_invalid_not_false() {
        let error = BranchPlanner::plan(
            &snapshot(),
            &command(vec![candidate("a", PredicateValue::Null, false)]),
        )
        .unwrap_err();
        assert!(matches!(error, BranchError::InvalidDecision(_)));
    }

    #[test]
    fn unresolved_dependencies_are_waiting_not_no_match() {
        let mut command = command(vec![]);
        command.unresolved_dependencies = vec!["task-x.output".into()];
        assert_eq!(
            BranchPlanner::plan(&snapshot(), &command),
            Ok(BranchPlan::Deferred {
                dependencies: vec!["task-x.output".into()]
            })
        );
    }
    #[test]
    fn structural_error_is_reported_before_dependency_deferral() {
        let mut command = command(vec![
            candidate("a", PredicateValue::False, true),
            candidate("b", PredicateValue::False, true),
        ]);
        command.unresolved_dependencies = vec!["vars.customer".into()];
        assert!(matches!(
            BranchPlanner::plan(&snapshot(), &command),
            Err(BranchError::InvalidDecision(_))
        ));
    }

    #[test]
    fn stale_predicate_basis_cannot_commit_against_a_newer_snapshot() {
        let mut command = command(vec![candidate("a", PredicateValue::True, false)]);
        command.predicate_basis.process -= 1;
        assert!(matches!(
            BranchPlanner::plan(&snapshot(), &command),
            Err(BranchError::StalePredicateBasis { .. })
        ));
    }
}
