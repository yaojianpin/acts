use super::{
    AggregateSnapshot, ExecutionGraph, ExecutionId, ExecutionState, OperationCommit, OperationId,
    OperationIntent, OperationKind, OutboxIntent, StateChange, TransitionPlan,
};
use std::fmt;

/// Join semantics are explicit model policy, never inferred from timing or from
/// whichever predecessor happens to finish last.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum JoinPolicy {
    /// Every predecessor must have a successful terminal outcome.
    AllSucceeded,
    /// Every predecessor merely has to be terminal. Used by explicit finally
    /// or reconciliation scopes, not by ordinary sequence/parallel flow.
    AllTerminal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PropagateCommand {
    pub(crate) process_id: String,
    pub(crate) execution: ExecutionId,
    pub(crate) source_operation: OperationCommit,
    pub(crate) join_policy: JoinPolicy,
}

pub(crate) struct PropagationPlanner;

impl PropagationPlanner {
    pub(crate) fn plan(
        graph: &ExecutionGraph,
        snapshot: &AggregateSnapshot,
        command: &PropagateCommand,
    ) -> Result<TransitionPlan, PropagationError> {
        validate_envelope(graph, snapshot, command)?;
        let source = snapshot
            .executions
            .get(&command.execution)
            .ok_or_else(|| PropagationError::ExecutionNotFound(command.execution.clone()))?;
        if !source.state.is_terminal() {
            return Err(PropagationError::SourceNotTerminal {
                execution: command.execution.clone(),
                state: source.state,
            });
        }

        let mut state_changes = Vec::new();
        let mut operations = Vec::new();
        // Operation ids are unique per (task, generation): a successor that
        // already ran hops (a fan-out child, a rewound generation) owns
        // earlier sequences, so every seeded operation takes the next one.
        let next_sequence = |task_id: &str, generation: u64| {
            snapshot
                .operations
                .keys()
                .filter(|id| {
                    id.process_id == command.process_id
                        && id.task_id == task_id
                        && id.generation == generation
                })
                .map(|id| id.sequence)
                .max()
                .map_or(0, |sequence| sequence + 1)
        };
        for successor in graph.successors(&command.execution) {
            let target = snapshot
                .executions
                .get(successor)
                .ok_or_else(|| PropagationError::ExecutionNotFound(successor.clone()))?;
            if target.state != ExecutionState::Created {
                // A committed successor is already scheduled or finished. The
                // source operation may still be completing after recovery, so
                // this is idempotent rather than an error — except when the
                // source is dead: an admitted-but-unfinished successor
                // (`Ready` queued for its start hop, `Running`/`Waiting` mid
                // execution, e.g. a branch child of a cancelled container)
                // joins the skip cascade in place instead of running on
                // behind a dead path.
                if !is_success(source.state)
                    && matches!(
                        target.state,
                        ExecutionState::Ready | ExecutionState::Running | ExecutionState::Waiting
                    )
                {
                    // The hop owns the state change (it re-reads the
                    // authoritative state); the plan only schedules it.
                    operations.push(OperationIntent {
                        id: OperationId {
                            process_id: command.process_id.clone(),
                            task_id: successor.task_id.clone(),
                            generation: successor.generation,
                            sequence: next_sequence(&successor.task_id, successor.generation),
                        },
                        kind: OperationKind::SkipTask,
                    });
                    tracing::debug!(
                        successor = ?successor,
                        source_state = ?source.state,
                        "dead source; admitted successor joins the skip cascade"
                    );
                }
                continue;
            }
            let readiness = readiness(graph, snapshot, successor, command.join_policy)?;
            match readiness {
                JoinReadiness::Ready => {
                    state_changes.push(StateChange {
                        execution: successor.clone(),
                        expected_revision: target.revision,
                        from: ExecutionState::Created,
                        to: ExecutionState::Ready,
                    });
                    operations.push(OperationIntent {
                        id: OperationId {
                            process_id: command.process_id.clone(),
                            task_id: successor.task_id.clone(),
                            generation: successor.generation,
                            sequence: next_sequence(&successor.task_id, successor.generation),
                        },
                        kind: OperationKind::ExecuteTask,
                    });
                }
                JoinReadiness::Waiting => {}
                // A dead predecessor (cancelled/failed/aborted) never blocks
                // the run: the successor is admitted and immediately handed a
                // skip cascade, which flows the deadness past it one hop per
                // operation.
                JoinReadiness::SkipCascade { predecessor, state } => {
                    state_changes.push(StateChange {
                        execution: successor.clone(),
                        expected_revision: target.revision,
                        from: ExecutionState::Created,
                        to: ExecutionState::Ready,
                    });
                    operations.push(OperationIntent {
                        id: OperationId {
                            process_id: command.process_id.clone(),
                            task_id: successor.task_id.clone(),
                            generation: successor.generation,
                            sequence: next_sequence(&successor.task_id, successor.generation),
                        },
                        kind: OperationKind::SkipTask,
                    });
                    tracing::debug!(
                        successor = ?successor,
                        predecessor = ?predecessor,
                        predecessor_state = ?state,
                        "join predecessor is dead; successor enters the skip cascade"
                    );
                }
            }
        }

        let plan = TransitionPlan {
            process_id: command.process_id.clone(),
            expected: snapshot.version.clone(),
            execution_creates: Vec::new(),
            state_changes,
            variable_writes: Vec::new(),
            source_operation: Some(command.source_operation.clone()),
            operations,
            outbox: vec![OutboxIntent {
                key: format!(
                    "propagate:{}:{}:{}:{}",
                    command.process_id,
                    command.execution.task_id,
                    command.execution.generation,
                    command.source_operation.id.sequence,
                ),
                topic: "task.propagated".into(),
                payload: format!(
                    "{}:{}",
                    command.execution.task_id, command.execution.generation
                ),
            }],
        };
        plan.validate()
            .map_err(|error| PropagationError::InvalidPlan(error.to_string()))?;
        Ok(plan)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum JoinReadiness {
    Ready,
    Waiting,
    SkipCascade {
        predecessor: ExecutionId,
        state: ExecutionState,
    },
}

fn readiness(
    graph: &ExecutionGraph,
    snapshot: &AggregateSnapshot,
    successor: &ExecutionId,
    policy: JoinPolicy,
) -> Result<JoinReadiness, PropagationError> {
    let predecessors = graph.predecessors(successor);
    if predecessors.is_empty() {
        return Ok(JoinReadiness::Ready);
    }
    let mut waiting = false;
    for predecessor in predecessors {
        let state = snapshot
            .executions
            .get(predecessor)
            .ok_or_else(|| PropagationError::ExecutionNotFound(predecessor.clone()))?
            .state;
        if !state.is_terminal() {
            waiting = true;
            continue;
        }
        if policy == JoinPolicy::AllSucceeded && !is_success(state) {
            // A dead terminal predecessor (cancelled/failed/aborted/skipped)
            // is decisive: the successor is skipped even if other
            // predecessors are still in flight — the flow is already over on
            // this path.
            return Ok(JoinReadiness::SkipCascade {
                predecessor: predecessor.clone(),
                state,
            });
        }
    }
    Ok(if waiting {
        JoinReadiness::Waiting
    } else {
        JoinReadiness::Ready
    })
}

/// The terminal states a join accepts as "this predecessor is done and the
/// flow may continue". `Skipped` is deliberately absent: a skipped
/// predecessor means the path is dead (a cancellation cascade flowed through
/// it), so its successors are skipped too instead of running.
fn is_success(state: ExecutionState) -> bool {
    matches!(state, ExecutionState::Completed | ExecutionState::Submitted)
}

fn validate_envelope(
    graph: &ExecutionGraph,
    snapshot: &AggregateSnapshot,
    command: &PropagateCommand,
) -> Result<(), PropagationError> {
    if graph.process_id != command.process_id || snapshot.process_id != command.process_id {
        return Err(PropagationError::WrongAggregate);
    }
    if graph.revision != snapshot.version.graph {
        return Err(PropagationError::GraphVersionConflict {
            graph: graph.revision,
            snapshot: snapshot.version.graph,
        });
    }
    let source = &command.source_operation.id;
    if source.process_id != command.process_id
        || source.task_id != command.execution.task_id
        || source.generation != command.execution.generation
    {
        return Err(PropagationError::SourceOperationTargetsDifferentExecution);
    }
    if graph.node(&command.execution).is_none() {
        return Err(PropagationError::ExecutionNotInGraph(
            command.execution.clone(),
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PropagationError {
    WrongAggregate,
    GraphVersionConflict {
        graph: u64,
        snapshot: u64,
    },
    SourceOperationTargetsDifferentExecution,
    ExecutionNotInGraph(ExecutionId),
    ExecutionNotFound(ExecutionId),
    SourceNotTerminal {
        execution: ExecutionId,
        state: ExecutionState,
    },
    InvalidPlan(String),
}
impl fmt::Display for PropagationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "propagation error: {self:?}")
    }
}
impl std::error::Error for PropagationError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::{
        AggregateVersion, ClaimToken, ExecutionGraphBuilder, ExecutionNode, ExecutionSnapshot,
        NodeId,
    };
    use std::collections::{BTreeMap, HashMap};

    fn id(name: &str) -> ExecutionId {
        ExecutionId::new(name, 0).unwrap()
    }
    fn node(name: &str) -> ExecutionNode {
        ExecutionNode {
            id: id(name),
            model_node: NodeId::parse(name).unwrap(),
            state: ExecutionState::Created,
            revision: 0,
        }
    }
    fn graph() -> ExecutionGraph {
        let mut b = ExecutionGraphBuilder::new("p1", 4).unwrap();
        for name in ["root", "a", "b", "join"] {
            b.add_node(node(name)).unwrap();
        }
        b.set_root(id("root")).unwrap();
        for name in ["a", "b", "join"] {
            b.add_child(id("root"), id(name)).unwrap();
        }
        b.add_successor(id("a"), id("join")).unwrap();
        b.add_successor(id("b"), id("join")).unwrap();
        b.build().unwrap()
    }
    fn snapshot(a: ExecutionState, b: ExecutionState) -> AggregateSnapshot {
        AggregateSnapshot {
            process_id: "p1".into(),
            version: AggregateVersion {
                graph: 4,
                process: 4,
                operations: 2,
            },
            executions: HashMap::from([
                (
                    id("root"),
                    ExecutionSnapshot {
                        model_node: NodeId::parse("node").unwrap(),
                        parent: None,
                        predecessors: vec![],
                        state: ExecutionState::Running,
                        revision: 0,
                    },
                ),
                (
                    id("a"),
                    ExecutionSnapshot {
                        model_node: NodeId::parse("node").unwrap(),
                        parent: None,
                        predecessors: vec![],
                        state: a,
                        revision: 1,
                    },
                ),
                (
                    id("b"),
                    ExecutionSnapshot {
                        model_node: NodeId::parse("node").unwrap(),
                        parent: None,
                        predecessors: vec![],
                        state: b,
                        revision: 1,
                    },
                ),
                (
                    id("join"),
                    ExecutionSnapshot {
                        model_node: NodeId::parse("node").unwrap(),
                        parent: None,
                        predecessors: vec![],
                        state: ExecutionState::Created,
                        revision: 0,
                    },
                ),
            ]),
            variables: BTreeMap::new(),
            operations: HashMap::new(),
            outbox: HashMap::new(),
        }
    }
    fn command(source: &str, policy: JoinPolicy) -> PropagateCommand {
        PropagateCommand {
            process_id: "p1".into(),
            execution: id(source),
            join_policy: policy,
            source_operation: OperationCommit {
                id: OperationId {
                    process_id: "p1".into(),
                    task_id: source.into(),
                    generation: 0,
                    sequence: 1,
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
    fn join_waits_until_every_predecessor_is_terminal() {
        let plan = PropagationPlanner::plan(
            &graph(),
            &snapshot(ExecutionState::Completed, ExecutionState::Running),
            &command("a", JoinPolicy::AllSucceeded),
        )
        .unwrap();
        assert!(plan.state_changes.is_empty());
        assert!(plan.operations.is_empty());
    }

    #[test]
    fn last_successful_predecessor_schedules_join_once() {
        let plan = PropagationPlanner::plan(
            &graph(),
            &snapshot(ExecutionState::Completed, ExecutionState::Completed),
            &command("b", JoinPolicy::AllSucceeded),
        )
        .unwrap();
        assert_eq!(plan.state_changes.len(), 1);
        assert_eq!(plan.state_changes[0].execution, id("join"));
        assert_eq!(plan.operations[0].kind, OperationKind::ExecuteTask);
    }

    #[test]
    fn dead_predecessor_puts_success_join_into_the_skip_cascade() {
        let plan = PropagationPlanner::plan(
            &graph(),
            &snapshot(ExecutionState::Failed, ExecutionState::Completed),
            &command("b", JoinPolicy::AllSucceeded),
        )
        .unwrap();
        // The join is admitted and handed a skip cascade: its one operation
        // is a SkipTask, not an execution.
        assert_eq!(plan.state_changes.len(), 1);
        assert_eq!(plan.state_changes[0].execution, id("join"));
        assert_eq!(plan.state_changes[0].to, ExecutionState::Ready);
        assert_eq!(plan.operations.len(), 1);
        assert_eq!(plan.operations[0].kind, OperationKind::SkipTask);
        assert_eq!(plan.operations[0].id.task_id, "join");
    }

    #[test]
    fn all_terminal_policy_allows_reconciliation_join() {
        let plan = PropagationPlanner::plan(
            &graph(),
            &snapshot(ExecutionState::Failed, ExecutionState::Cancelled),
            &command("b", JoinPolicy::AllTerminal),
        )
        .unwrap();
        assert_eq!(plan.state_changes[0].to, ExecutionState::Ready);
    }

    #[test]
    fn dead_source_sends_admitted_unfinished_successors_into_the_cascade() {
        // `join` is already `Ready` (admitted, start hop pending) when its
        // predecessor is cancelled: it must skip, not run behind a dead path.
        let mut snap = snapshot(ExecutionState::Cancelled, ExecutionState::Completed);
        if let Some(target) = snap.executions.get_mut(&id("join")) {
            target.state = ExecutionState::Ready;
        }
        let plan =
            PropagationPlanner::plan(&graph(), &snap, &command("a", JoinPolicy::AllSucceeded))
                .unwrap();
        // A `Ready` target needs no admission change, just the skip operation.
        assert!(plan.state_changes.is_empty());
        assert_eq!(plan.operations.len(), 1);
        assert_eq!(plan.operations[0].kind, OperationKind::SkipTask);
        assert_eq!(plan.operations[0].id.task_id, "join");
    }

    #[test]
    fn a_running_successor_of_a_dead_source_is_skipped_in_place() {
        let mut b = ExecutionGraphBuilder::new("p1", 4).unwrap();
        for name in ["root", "a", "b"] {
            b.add_node(ExecutionNode {
                id: id(name),
                model_node: NodeId::parse(name).unwrap(),
                state: ExecutionState::Created,
                revision: 0,
            })
            .unwrap();
        }
        b.set_root(id("root")).unwrap();
        b.add_child(id("root"), id("a")).unwrap();
        b.add_child(id("root"), id("b")).unwrap();
        b.add_successor(id("a"), id("b")).unwrap();
        let graph = b.build().unwrap();

        let mut snapshot = snapshot(ExecutionState::Cancelled, ExecutionState::Running);
        if let Some(target) = snapshot.executions.get_mut(&id("b")) {
            target.revision = 6;
        }
        let plan =
            PropagationPlanner::plan(&graph, &snapshot, &command("a", JoinPolicy::AllSucceeded))
                .unwrap();
        // The plan only schedules the skip; the hop owns the state change
        // against the authoritative state.
        assert!(plan.state_changes.is_empty());
        assert_eq!(plan.operations.len(), 1);
        assert_eq!(plan.operations[0].kind, OperationKind::SkipTask);
        assert_eq!(plan.operations[0].id.task_id, "b");
        assert_eq!(plan.operations[0].id.sequence, 0, "no prior hops on b");
    }

    #[test]
    fn a_live_source_leaves_admitted_successors_alone() {
        let plan = PropagationPlanner::plan(
            &graph(),
            &snapshot(ExecutionState::Completed, ExecutionState::Ready),
            &command("a", JoinPolicy::AllSucceeded),
        )
        .unwrap();
        assert!(plan.state_changes.is_empty());
        assert!(plan.operations.is_empty());
    }
}
