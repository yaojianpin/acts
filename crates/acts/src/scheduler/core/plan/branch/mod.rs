use crate::scheduler::core::{
    commit::aggregate::ProcessSnapshot,
    model::{model_graph::NodeId, task_graph::TaskId, transition_plan::OperationCommit},
};

use crate::scheduler::core::plan::ActPlan;

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
    pub(crate) node: String,
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
    pub(crate) branch: TaskId,
    pub(crate) candidates: Vec<BranchCandidate>,
    /// Process version against which every predicate value was evaluated.
    pub(crate) predicate_basis: PredicateBasis,
    pub(crate) unresolved_dependencies: Vec<String>,
    pub(crate) source_operation: OperationCommit,
}

pub(crate) struct Decide;

impl ActPlan for Decide {
    type Reading<'a> = ();
    type Command<'a> = BranchCommand;
    type Output = BranchDecision;
    type Error = std::convert::Infallible;

    fn plan(
        _reading: Self::Reading<'_>,
        command: &Self::Command<'_>,
    ) -> Result<Self::Output, Self::Error> {
        if let Err(error) = Self::validate_command(command) {
            return Ok(BranchDecision::Invalid {
                message: error.to_string(),
            });
        }
        Ok(Self::select(
            &command.candidates,
            &command.unresolved_dependencies,
        ))
    }
}

impl Decide {
    /// Validate the command's structural facts before considering dependency
    /// deferral. An invalid model must never masquerade as healthy waiting.
    fn validate_command(command: &BranchCommand) -> Result<(), BranchError> {
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
            if candidate.node.trim().is_empty() {
                return Err(BranchError::InvalidTargetTaskId(candidate.node.clone()));
            }
            if candidate.is_else {
                continue;
            }
            match &candidate.predicate {
                PredicateValue::True | PredicateValue::False => {}
                PredicateValue::Null => {
                    return Err(BranchError::InvalidDecision(format!(
                        "branch '{}' predicate evaluated to null",
                        candidate.node
                    )));
                }
                PredicateValue::Invalid { message } => {
                    return Err(BranchError::InvalidDecision(message.clone()));
                }
            }
        }
        Ok(())
    }

    /// The pure selection over already-validated candidates: every `true`
    /// predicate runs, the `else` candidate runs only when no `if` matched,
    /// and an unmatched set without `else` is an explicit `NoMatch`.
    /// Dependency deferral is layered on top by the caller.
    fn select(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::core::plan::ActPlan;

    fn candidate(node: &str, predicate: PredicateValue, is_else: bool) -> BranchCandidate {
        BranchCandidate {
            node: node.into(),
            model_node: NodeId::parse(node).unwrap(),
            predicate,
            is_else,
        }
    }

    fn command(candidates: Vec<BranchCandidate>) -> BranchCommand {
        BranchCommand {
            process_id: "p".into(),
            branch: TaskId::new("branch", 0).unwrap(),
            candidates,
            predicate_basis: PredicateBasis {
                graph: 1,
                process: 2,
            },
            unresolved_dependencies: Vec::new(),
            source_operation: OperationCommit {
                id: crate::scheduler::core::model::operation::OperationId {
                    process_id: "p".into(),
                    task: TaskId::new("branch", 0).unwrap(),
                    sequence: 1,
                },
                claim: crate::scheduler::core::model::operation::ClaimToken {
                    owner: "w".into(),
                    fence: 1,
                    lease_until_ms: 10,
                    heartbeat_revision: 0,
                },
            },
        }
    }

    #[test]
    fn true_predicates_run_together_and_the_else_is_the_fallback() {
        let selected = Decide::plan(
            (),
            &command(vec![
                candidate("a", PredicateValue::True, false),
                candidate("b", PredicateValue::False, false),
                candidate("catch", PredicateValue::False, true),
            ]),
        )
        .unwrap();
        assert_eq!(
            selected,
            BranchDecision::Selected(vec![candidate("a", PredicateValue::True, false)])
        );

        // No true predicate: the else branch is the only selection.
        let fallback = Decide::plan(
            (),
            &command(vec![
                candidate("a", PredicateValue::False, false),
                candidate("catch", PredicateValue::Null, true),
            ]),
        )
        .unwrap();
        assert_eq!(
            fallback,
            BranchDecision::Selected(vec![candidate("catch", PredicateValue::Null, true)])
        );

        // Neither: an explicit no-match, not an empty selection.
        assert_eq!(
            Decide::plan(
                (),
                &command(vec![candidate("a", PredicateValue::False, false)])
            )
            .unwrap(),
            BranchDecision::NoMatch
        );
    }

    #[test]
    fn unresolved_dependencies_are_sorted_and_deduplicated() {
        let mut command = command(vec![candidate("a", PredicateValue::True, false)]);
        command.unresolved_dependencies = vec!["z".into(), "a".into(), "z".into()];
        assert_eq!(
            Decide::plan((), &command).unwrap(),
            BranchDecision::Waiting {
                dependencies: vec!["a".into(), "z".into()]
            }
        );
    }

    #[test]
    fn an_invalid_model_is_refused_instead_of_waiting() {
        // Two else branches.
        let invalid = Decide::plan(
            (),
            &command(vec![
                candidate("a", PredicateValue::False, true),
                candidate("b", PredicateValue::False, true),
            ]),
        )
        .unwrap();
        assert!(matches!(invalid, BranchDecision::Invalid { .. }));

        // A blank node id (the candidate was built by a caller, so its
        // model_node is still a valid id).
        let blank = BranchCandidate {
            node: "  ".into(),
            model_node: NodeId::parse("blank").unwrap(),
            predicate: PredicateValue::True,
            is_else: false,
        };
        let invalid = Decide::plan((), &command(vec![blank])).unwrap();
        assert!(matches!(invalid, BranchDecision::Invalid { .. }));

        // A null predicate on an if branch.
        let invalid = Decide::plan(
            (),
            &command(vec![candidate("a", PredicateValue::Null, false)]),
        )
        .unwrap();
        assert_eq!(
            invalid,
            BranchDecision::Invalid {
                message:
                    "branch error: InvalidDecision(\"branch 'a' predicate evaluated to null\")"
                        .into()
            }
        );

        // A predicate that could not be evaluated carries its own message.
        let invalid = Decide::plan(
            (),
            &command(vec![candidate(
                "a",
                PredicateValue::Invalid {
                    message: "bad expression".into(),
                },
                false,
            )]),
        )
        .unwrap();
        assert_eq!(
            invalid,
            BranchDecision::Invalid {
                message: "branch error: InvalidDecision(\"bad expression\")".into()
            }
        );
    }

    #[test]
    fn branch_errors_render() {
        assert!(
            BranchError::InvalidDecision("x".into())
                .to_string()
                .starts_with("branch error:")
        );
        assert!(
            BranchError::InvalidTargetTaskId("x".into())
                .to_string()
                .starts_with("branch error:")
        );
    }
}
