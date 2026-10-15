use crate::scheduler::core::{
    commit::process::ProcessSnapshot,
    model::{execution_graph::ExecutionId, model_graph::NodeId, transition_plan::OperationCommit},
};

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
    pub(crate) fn from_snapshot(snapshot: &ProcessSnapshot) -> Self {
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
    /// Process version against which every predicate value was evaluated.
    pub(crate) predicate_basis: PredicateBasis,
    pub(crate) unresolved_dependencies: Vec<String>,
    pub(crate) source_operation: OperationCommit,
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
        Self::select(&command.candidates, &command.unresolved_dependencies)
    }

    /// The pure selection over already-validated candidates: every `true`
    /// predicate runs, the `else` candidate runs only when no `if` matched,
    /// and an unmatched set without `else` is an explicit `NoMatch`.
    /// Dependency deferral is layered on top by the caller.
    pub(crate) fn select(
        candidates: &[BranchCandidate],
        unresolved_dependencies: &[String],
    ) -> BranchDecision {
        if !unresolved_dependencies.is_empty() {
            let mut dependencies = unresolved_dependencies.to_vec();
            dependencies.sort();
            dependencies.dedup();
            return BranchDecision::Waiting { dependencies };
        }
        let mut selected = Vec::new();
        let mut fallback = None;
        for candidate in candidates {
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
}

mod errors;

pub(crate) use errors::*;
