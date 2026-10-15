use crate::scheduler::core::{
    commit::aggregate::AggregateSnapshot,
    model::{
        execution_graph::{ExecutionGraph, ExecutionId, ExecutionState},
        model_graph::NodeId,
        operation::{OperationId, OperationKind},
        transition_plan::{
            ExecutionCreate, OperationCommit, OperationIntent, OutboxIntent, StateChange,
            TransitionPlan, VariableWrite,
        },
    },
};

use std::fmt;

/// Join semantics are explicit model policy, never inferred from timing or from
/// whichever predecessor happens to finish last.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum JoinPolicy {
    /// Every predecessor must have a successful terminal outcome.
    AllSucceeded,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PropagateCommand {
    pub(crate) process_id: String,
    pub(crate) execution: ExecutionId,
    pub(crate) source_operation: OperationCommit,
    pub(crate) join_policy: JoinPolicy,
    /// The flow the host read from the declaring model, when the settling
    /// execution is a *step*: the node the flow goes to and whether it must be
    /// re-created first because its newest generation is already terminal.
    ///
    /// The graph's successors only encode the chains the seed and the
    /// container fan-outs wired (a declaration chain per container), while a
    /// step's flow is a *model* fact: its explicit `next` may jump outside the
    /// graph (backward to an ancestor, forward over a sibling), and a `while`
    /// step loops back into itself. `None` for a node the model does not name
    /// (a branch, a handler container, the workflow root), whose flow is the
    /// graph's own edges.
    pub(crate) flow: Option<FlowTarget>,
}

/// Where a step's flow goes once it settled — see [`PropagateCommand::flow`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FlowTarget {
    /// The model node the flow enters.
    pub(crate) node: String,
    /// Its newest generation is already terminal: the flow re-enters the node,
    /// so a fresh generation is created (a backward `next`, a `while` turn).
    pub(crate) reopen: bool,
    /// The node's durable auto decision (`__auto`) — what a re-entered
    /// generation's scope must carry, like every fan-out child does, or the
    /// engine would treat a waiting act as a self-completing one.
    pub(crate) auto: bool,
    /// The node's declared model variables (JSON text), seeded on the
    /// re-entered generation's own scope — the same fact the fan-out seeds on
    /// a child (`child_variables`), which the seed only wrote for generation 0.
    pub(crate) vars: Vec<(String, String)>,
}

/// The private variable an execution carries when its own `if`/`while` guard
/// decided it `Skipped`: the guard *fell through* — the legacy step skipped by
/// its condition, whose flow goes on to the next declared step — as opposed to
/// a skip the deadness cascade flowed in. A `__` fact: out of the expression
/// space and the read model, durable so a replay reads the same verdict.
pub(crate) const GUARD_SKIP_VAR: &str = "__guard_skip";

/// The engine's failure vocabulary on a scope: the names the catch fan-out
/// reserves and the upward error walk carries (`ecode`/`message` are the
/// legacy task-data names, `error.code`/`error.message` the kernel's own).
fn is_error_fact(name: &str) -> bool {
    matches!(name, "ecode" | "message" | "error.code" | "error.message")
}

/// Whether one execution was skipped by its own guard (see [`GUARD_SKIP_VAR`]).
pub(crate) fn is_guard_skip(snapshot: &AggregateSnapshot, execution: &ExecutionId) -> bool {
    snapshot
        .variables
        .contains_key(&(execution.clone(), GUARD_SKIP_VAR.to_string()))
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
        let mut creates = Vec::new();
        let mut writes: Vec<VariableWrite> = Vec::new();
        // Whether the source's death flows on as a cascade (a dead source's
        // admitted successors join it) or the source is merely *done*: a step
        // its own guard skipped is the legacy fall-through — its flow runs on
        // into the next declared step.
        let source_alive = is_success(source.state) || is_guard_skip(snapshot, &command.execution);
        // The targets this propagation flows into: the one node the model
        // names when the host decided the flow (see [`PropagateCommand::flow`]),
        // otherwise the graph's own successors.
        let targets: Vec<ExecutionId> = match &command.flow {
            Some(flow) => {
                let Some(latest) = newest_generation(snapshot, &flow.node) else {
                    // A `next` naming a node the run never instantiated: the
                    // model is inconsistent with the seeded graph. Refusing is
                    // the only honest answer — scheduling nothing would end
                    // the flow silently.
                    return Err(PropagationError::FlowTargetNotInstantiated(
                        flow.node.clone(),
                    ));
                };
                if flow.reopen {
                    // The flow re-enters a node that was already activated —
                    // terminal (a backward `next`, a `while` step's next turn)
                    // or even still parked for its children (the flow's own
                    // jump back into an ancestor while it waits). Only the node
                    // itself is created here — its own
                    // hops bring its forward closure alive (a container fans
                    // its children out as fresh generations), exactly like the
                    // seeded chain — with the parent and predecessors the dead
                    // generation had, so the join keeps waiting where it did.
                    let generation = latest
                        .0
                        .generation
                        .checked_add(1)
                        .ok_or(PropagationError::GenerationOverflow)?;
                    let execution = ExecutionId::new(flow.node.clone(), generation)
                        .map_err(|_| PropagationError::EmptyFlowTarget)?;
                    // The re-entered generation inherits the dead one's edges,
                    // and it is admitted like any other execution: `Ready` with
                    // its start hop when every predecessor it inherited has
                    // settled, otherwise `Created` — the predecessor still in
                    // flight admits it through its own propagation (its
                    // successors include this execution the moment the create
                    // commits).
                    // A predecessor that *is* the parent container counts as
                    // passed: the parent parks precisely to wait for its
                    // children, and the flow re-enters inside it (the seeded
                    // chain's head is admitted the same way, as the fan-out's
                    // `Entry` child).
                    let predecessors_ready = latest.1.predecessors.iter().all(|predecessor| {
                        Some(predecessor) == latest.1.parent.as_ref()
                            || snapshot
                                .executions
                                .get(predecessor)
                                .is_some_and(|execution| execution.state.is_terminal())
                    });
                    creates.push(ExecutionCreate {
                        execution: execution.clone(),
                        model_node: NodeId::parse(flow.node.clone())
                            .map_err(|_| PropagationError::EmptyFlowTarget)?,
                        parent: latest.1.parent.clone(),
                        predecessors: latest.1.predecessors.clone(),
                        initial_state: if predecessors_ready {
                            ExecutionState::Ready
                        } else {
                            ExecutionState::Created
                        },
                    });
                    if predecessors_ready {
                        operations.push(OperationIntent {
                            id: OperationId {
                                process_id: command.process_id.clone(),
                                task_id: flow.node.clone(),
                                generation,
                                sequence: snapshot.next_operation_sequence(&flow.node, generation),
                            },
                            kind: OperationKind::ExecuteTask,
                        });
                    }
                    // The re-entered generation's own scope carries what every
                    // other execution's does: its auto decision and its declared
                    // model variables.
                    writes.push(VariableWrite {
                        scope: execution.clone(),
                        name: "auto".to_string(),
                        value: if flow.auto { "1" } else { "0" }.to_string(),
                    });
                    for (name, value) in &flow.vars {
                        writes.push(VariableWrite {
                            scope: execution.clone(),
                            name: name.clone(),
                            value: value.clone(),
                        });
                    }
                    // The flow carries the source's own data into the scope the
                    // re-entered node inherits from — the legacy `Task::next`
                    // folding `parent.update_data(&outputs)` up the chain
                    // *before* the next task reads it. Without this a jump back
                    // to an ancestor would re-evaluate the ancestor's predicates
                    // against the state from before the iteration that asked for
                    // it (the iteration's own fold only reaches its immediate
                    // parent).
                    if let Some(parent) = latest.1.parent.clone() {
                        for ((scope, name), value) in &snapshot.variables {
                            if scope != &command.execution
                                || crate::utils::consts::is_private_key(name)
                                || name == "auto"
                                // The failure vocabulary travels up the
                                // ancestor walk of its own hop (`plan_upward`'s
                                // carry), which writes it to every ancestor it
                                // decides: carrying it here too would fight
                                // that walk over the same scope.
                                || is_error_fact(name)
                                || writes.iter().any(|write| {
                                    write.scope == parent && write.name == *name
                                })
                            {
                                continue;
                            }
                            writes.push(VariableWrite {
                                scope: parent.clone(),
                                name: name.clone(),
                                value: value.clone(),
                            });
                        }
                    }
                    tracing::debug!(
                        node = %flow.node,
                        generation,
                        "the flow re-enters a settled node"
                    );
                    Vec::new()
                } else {
                    vec![latest.0]
                }
            }
            None => graph.successors(&command.execution).to_vec(),
        };
        for successor in &targets {
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
                if !source_alive
                    && matches!(
                        target.state,
                        ExecutionState::Ready | ExecutionState::Running | ExecutionState::Waiting
                    )
                {
                    // The hop owns the state change (it re-reads the
                    // authoritative state); the plan only schedules it. An
                    // aborted source's `Running` successor is `Aborted` (the
                    // legacy ancestor walk aborts running children); every
                    // other death skips its successors.
                    let kind = if source.state == ExecutionState::Aborted
                        && target.state == ExecutionState::Running
                    {
                        OperationKind::AbortTask
                    } else {
                        OperationKind::SkipTask
                    };
                    operations.push(OperationIntent {
                        id: OperationId {
                            process_id: command.process_id.clone(),
                            task_id: successor.task_id.clone(),
                            generation: successor.generation,
                            sequence: snapshot
                                .next_operation_sequence(&successor.task_id, successor.generation),
                        },
                        kind,
                    });
                    tracing::debug!(
                        successor = ?successor,
                        source_state = ?source.state,
                        kind = ?kind,
                        "dead source; admitted successor joins the cascade"
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
                            sequence: snapshot
                                .next_operation_sequence(&successor.task_id, successor.generation),
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
                            sequence: snapshot
                                .next_operation_sequence(&successor.task_id, successor.generation),
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
            execution_creates: creates,
            state_changes,
            variable_writes: writes,
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
                receipt: None,
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
        let execution = snapshot
            .executions
            .get(predecessor)
            .ok_or_else(|| PropagationError::ExecutionNotFound(predecessor.clone()))?;
        let state = execution.state;
        if !state.is_terminal() {
            waiting = true;
            continue;
        }
        // A step its own `if`/`while` guard skipped is the legacy
        // fall-through, not a dead path: its successor runs (the guard said
        // "not now", not "everything below is dead").
        if state == ExecutionState::Skipped && is_guard_skip(snapshot, predecessor) {
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

/// The newest generation of one model node, with its snapshot. `None` when the
/// run never instantiated it.
fn newest_generation<'a>(
    snapshot: &'a AggregateSnapshot,
    node: &str,
) -> Option<(
    ExecutionId,
    &'a crate::scheduler::core::commit::aggregate::ExecutionSnapshot,
)> {
    snapshot
        .executions
        .iter()
        .filter(|(id, _)| id.task_id == node)
        .max_by_key(|(id, _)| id.generation)
        .map(|(id, execution)| (id.clone(), execution))
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
    /// A step's `next` names a node the run never instantiated (the model and
    /// the seeded graph disagree). Scheduling nothing would end the flow
    /// silently, so the hop refuses instead.
    FlowTargetNotInstantiated(String),
    EmptyFlowTarget,
    GenerationOverflow,
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
    use crate::scheduler::core::commit::aggregate::ExecutionSnapshot;
    use crate::scheduler::core::model::execution_graph::ExecutionGraphBuilder;
    use crate::scheduler::core::model::execution_graph::ExecutionNode;
    use crate::scheduler::core::model::model_graph::NodeId;
    use crate::scheduler::core::model::operation::ClaimToken;
    use crate::scheduler::core::model::transition_plan::AggregateVersion;
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
            flow: None,
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
