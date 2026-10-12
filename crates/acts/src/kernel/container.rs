//! Container fan-out and join planning.
//!
//! A *container* execution holds model children: a step with `branches`, a
//! step with `catches`, a branch with its own `steps`, or the workflow root
//! over its seeded chain. Instead of completing at its effect point, a
//! container fans out — every child becomes an execution in one atomic plan —
//! and stays `Running` (in the `children` flow, see [`FLOW_VAR`]) until all of
//! its children are terminal (the join). That is the legacy engine's shape: a
//! step stays running while `check_in_children`'s children run and
//! `auto_complete` finishes it once they all settled.
//!
//! `Running` is the park state, not `Waiting`: `Waiting` is the legacy
//! `Interrupt` — an execution whose own act nobody answered yet — and a child's
//! settlement must never decide one of those.
//!
//! A failed step fans its `catches` out through the same plan but parks in
//! `Running`, and its join is the *catch* join ([`ContainerPlanner::plan_catch_join`]):
//! the legacy `Step::on_error` keeps the failed step running while the CATCH
//! children decide whether the error was caught, and `auto_complete` decides
//! it from their states.
//!
//! The planner is pure: it receives the snapshot, the already-evaluated
//! selection (branch predicates are the host's job, selection semantics are
//! [`super::BranchPlanner`]'s), and the per-child durable facts (`auto`,
//! declared vars). Admission of each child goes through
//! [`super::GenerationAdmissionPolicy`], so a re-entered container reuses the
//! generations a Cancel rewind already recreated instead of allocating
//! duplicates.

use super::{
    AggregateSnapshot, ExecutionCreate, ExecutionGraph, ExecutionId, ExecutionState,
    ExistingGeneration, GenerationAdmissionPolicy, NodeId, OperationCommit, OperationId,
    OperationIntent, OperationKind, OutboxIntent, SlotObservation, StateChange, TransitionPlan,
    VariableWrite,
};
use std::collections::HashMap;
use std::fmt;

/// The private variable an execution carries while it decides a caught error
/// (legacy `Sign::ERROR` on the failed step) and its CATCH children carry
/// (legacy `TASK_SIGN = CATCH`). Both roles share one key and two values: the
/// sign is durable plan state, so a replay — or the catch join — reads the
/// decision from the aggregate instead of from the declaring spec.
pub(crate) const SIGN_VAR: &str = "__sign";
/// A child the failed step dispatched as a CATCH handler.
///
/// The *role* fact: what this execution is to its parent. It is a separate axis
/// from [`FLOW_VAR`] (who decides this execution next), and the two must stay
/// separate keys — a handler is often itself a container, and its own fan-out
/// would otherwise overwrite the role its parent's join reads back from it.
pub(crate) const SIGN_CATCH: &str = "catch";

/// The private variable holding which decision flow an execution is in, when it
/// is not simply in its own act's flow (the flow fact's *absence*).
///
/// Two values, one axis:
///
/// - [`FLOW_CHILDREN`]: the execution admitted children and is `Running` while
///   they settle; the plain join ([`ContainerPlanner::plan_join`]) decides it.
/// - [`FLOW_CATCHES`]: its failure is durable and it dispatched its `catches`;
///   the catch join ([`ContainerPlanner::plan_catch_join`]) decides it — and
///   only that join, so a plain container join can never mistake a failed step
///   for one waiting on its children (or a child's settlement for a completed
///   step's).
///
/// Both values are needed as *durable* facts rather than derived from the
/// state: after the park moved to `Running`, "`Running` with children" means
/// either flow, and which one decides the execution is exactly the question a
/// replay has to answer.
///
/// `timeout` is deliberately **not** a value here. A timer runs *in parallel*
/// to the act's own flow (it fires while the act executes or waits), so a
/// single-valued flow key could not hold it without losing the act flow; the
/// accurate expression of "a timeout is pending" is the timer operation itself
/// ([`super::OperationKind::DispatchTimeout`]), which holds the cadence and is
/// re-woken by the recovery index.
pub(crate) const FLOW_VAR: &str = "__flow";
/// The value of [`FLOW_VAR`] on an execution waiting for the children it
/// admitted.
pub(crate) const FLOW_CHILDREN: &str = "children";
/// The value of [`FLOW_VAR`] on a failed execution deciding its `catches`.
pub(crate) const FLOW_CATCHES: &str = "catches";

/// Which decision flow one execution is in, beyond its own act's.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExecutionFlow {
    /// Waiting for the children it admitted: the plain join decides it.
    Children,
    /// Deciding its `catches`: the catch join decides it.
    Catches,
}

/// The flow fact ([`FLOW_VAR`]) of one execution, if any. `None` means the
/// execution is in its own act's flow — `Running` with it in flight, `Waiting`
/// for a client, or not started yet.
pub(crate) fn execution_flow(
    snapshot: &AggregateSnapshot,
    execution: &ExecutionId,
) -> Option<ExecutionFlow> {
    match snapshot
        .variables
        .get(&(execution.clone(), FLOW_VAR.to_string()))
        .map(String::as_str)
    {
        Some(FLOW_CHILDREN) => Some(ExecutionFlow::Children),
        Some(FLOW_CATCHES) => Some(ExecutionFlow::Catches),
        _ => None,
    }
}

/// Whether one execution is parked in `Running` for the children it admitted.
pub(crate) fn parked_for_children(snapshot: &AggregateSnapshot, execution: &ExecutionId) -> bool {
    execution_flow(snapshot, execution) == Some(ExecutionFlow::Children)
}

/// Whether one execution is deciding its `catches` (the legacy `Sign::ERROR`
/// on the failed step).
pub(crate) fn deciding_catches(snapshot: &AggregateSnapshot, execution: &ExecutionId) -> bool {
    execution_flow(snapshot, execution) == Some(ExecutionFlow::Catches)
}

/// The sign fact ([`SIGN_VAR`]) of one execution, if any.
pub(crate) fn execution_sign<'a>(
    snapshot: &'a AggregateSnapshot,
    execution: &ExecutionId,
) -> Option<&'a str> {
    snapshot
        .variables
        .get(&(execution.clone(), SIGN_VAR.to_string()))
        .map(String::as_str)
}

/// What a fan-out child is admitted as.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ContainerChildPlan {
    /// Runs now: created `Ready` (or readmitted from `Created`) with an
    /// `ExecuteTask` operation.
    Entry,
    /// Not selected: created `Ready` with a `SkipTask` operation, so the
    /// parity floor keeps one terminal execution per model node (the legacy
    /// engine keeps the skipped branch task).
    Skip,
    /// A chain successor: created `Created`; ordinary propagation admits it
    /// when its predecessor settles. An execution that already exists (the
    /// root's seeded chain, or a rewind's recreation) is left exactly as it
    /// is — the fan-out never readmits one, because the admission would hand
    /// it `Ready` with no operation to run.
    Queued,
}

/// One child execution to admit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ContainerChild {
    pub(crate) task_id: String,
    pub(crate) model_node: NodeId,
    pub(crate) plan: ContainerChildPlan,
    /// Edges of the created execution. `Source` is the fanning-out
    /// container; `Sibling(i)` the i-th child of the same command, so the
    /// host can chain children whose generations only the planner computes.
    pub(crate) predecessors: Vec<ContainerPredecessor>,
    /// Declared model vars, seeded as this execution's own scope (JSON
    /// strings, the aggregate's variable representation).
    pub(crate) vars: Vec<(String, String)>,
    pub(crate) auto: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ContainerPredecessor {
    Source,
    Sibling(usize),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ContainerFanoutCommand {
    pub(crate) process_id: String,
    pub(crate) source: ExecutionId,
    pub(crate) source_operation: OperationCommit,
    pub(crate) children: Vec<ContainerChild>,
    /// Client inputs of the completion that fans out (an auto completion has
    /// none). They land on the container's own scope, exactly like the
    /// action-variable writes of a plain completion.
    pub(crate) inputs: Vec<(String, String)>,
    /// Where the source parks while its children run: `Running` for every
    /// container — the legacy step stays running while `check_in_children`'s
    /// children run, `auto_complete` finishes it once they settled, and the
    /// failed step dispatching its CATCH children is running too
    /// (`Step::on_error`). A source that is `Waiting` when it fans out (its
    /// act was just answered) moves to `Running`, exactly like the legacy step
    /// resuming its `next`.
    ///
    /// `Waiting` stays a valid park value only for the timeout dispatch, which
    /// admits a branch under an execution that keeps waiting on its own act.
    pub(crate) source_park: ExecutionState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ContainerJoinCommand {
    pub(crate) process_id: String,
    pub(crate) parent: ExecutionId,
}

/// The join of a step that is deciding a caught error: its CATCH children
/// have settled, and [`ContainerPlanner::plan_catch_join`] decides the step.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CatchJoinCommand {
    pub(crate) process_id: String,
    /// The failed step whose CATCH children are settling.
    pub(crate) parent: ExecutionId,
    /// The CATCH child whose settlement triggered this join.
    pub(crate) source: ExecutionId,
}

/// What a settled CATCH child set decided.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CatchJoinOutcome {
    /// Every CATCH child skipped: no handler matched, so the error resumes
    /// (the legacy `auto_complete` re-derives it and calls `emit_error`). The
    /// step is decided `Failed` and propagates, which is what carries the
    /// error walk up.
    Resumed(StateChange, OperationIntent),
    /// At least one CATCH child did not skip: the error was caught, and the
    /// step completes (the legacy `set_state_if_running(Completed)`).
    Caught(StateChange, OperationIntent),
}

pub(crate) struct ContainerPlanner;

impl ContainerPlanner {
    /// Fan the container's children out in one atomic plan. The source parks
    /// in `command.source_park` — `Running` for every container (the legacy
    /// step stays running while its children run, and a failed step
    /// dispatching its CATCH children is running too); the join (see
    /// [`Self::plan_join`] /
    /// [`Self::plan_catch_join`]) decides it once every child is terminal. An
    /// empty container completes immediately.
    pub(crate) fn plan_fanout(
        snapshot: &AggregateSnapshot,
        command: &ContainerFanoutCommand,
    ) -> Result<TransitionPlan, ContainerError> {
        if snapshot.process_id != command.process_id {
            return Err(ContainerError::WrongAggregate);
        }
        if !matches!(
            command.source_park,
            ExecutionState::Running | ExecutionState::Waiting
        ) {
            return Err(ContainerError::InvalidSourcePark(command.source_park));
        }
        let source = snapshot
            .executions
            .get(&command.source)
            .ok_or_else(|| ContainerError::ExecutionNotFound(command.source.clone()))?;
        if !matches!(
            source.state,
            ExecutionState::Running | ExecutionState::Waiting
        ) {
            return Err(ContainerError::SourceNotContainer {
                source: command.source.clone(),
                state: source.state,
            });
        }
        let operation = &command.source_operation.id;
        if operation.process_id != command.process_id
            || operation.task_id != command.source.task_id
            || operation.generation != command.source.generation
        {
            return Err(ContainerError::SourceOperationTargetsDifferentExecution);
        }
        if command.children.is_empty() {
            // A container without children is a normal completion: the join
            // of zero children fires in the same hop.
            return Self::empty_container_plan(snapshot, command, source.state, source.revision);
        }

        let mut creates = Vec::new();
        // The source parks in `command.source_park` — unless a client
        // completion found it already parked, in which case the state is
        // already the parked one.
        let mut state_changes = Vec::new();
        if source.state != command.source_park {
            state_changes.push(StateChange {
                execution: command.source.clone(),
                expected_revision: source.revision,
                from: source.state,
                to: command.source_park,
            });
        }
        let mut variable_writes: Vec<VariableWrite> = command
            .inputs
            .iter()
            .map(|(name, value)| VariableWrite {
                scope: command.source.clone(),
                name: name.clone(),
                value: value.clone(),
            })
            .collect();
        let mut operations = Vec::new();
        // Resolved execution of each already-processed child, for sibling
        // predecessor references.
        let mut resolved: Vec<ExecutionId> = Vec::with_capacity(command.children.len());
        for (index, child) in command.children.iter().enumerate() {
            if child.task_id.trim().is_empty() {
                return Err(ContainerError::EmptyChildTaskId);
            }
            let observation = SlotObservation::vacant(false, false)
                .with_existing(existing_generation(snapshot, &child.task_id));
            let generation = next_generation(snapshot, &child.task_id)
                .ok_or(ContainerError::GenerationOverflow)?;
            let execution = match super::GenerationAdmissionPolicy::decide(
                super::ExecutionSlotKind::Once,
                super::AdmissionIntent::Forward,
                observation,
            ) {
                super::AdmissionDecision::Ignore => {
                    resolved.push(
                        latest_execution(snapshot, &child.task_id)
                            .ok_or(ContainerError::ExecutionNotFound(command.source.clone()))?,
                    );
                    continue;
                }
                super::AdmissionDecision::AllocateGeneration => {
                    let execution = ExecutionId::new(child.task_id.clone(), generation)
                        .map_err(|_| ContainerError::EmptyChildTaskId)?;
                    creates.push(ExecutionCreate {
                        execution: execution.clone(),
                        model_node: child.model_node.clone(),
                        parent: Some(command.source.clone()),
                        predecessors: resolve_predecessors(
                            &child.predecessors,
                            &command.source,
                            &resolved,
                            index,
                        )?,
                        initial_state: match child.plan {
                            ContainerChildPlan::Queued => ExecutionState::Created,
                            ContainerChildPlan::Entry | ContainerChildPlan::Skip => {
                                ExecutionState::Ready
                            }
                        },
                    });
                    if let Some(intent) = child_operation(command, &execution, child.plan)? {
                        operations.push(intent);
                    }
                    child_variables(&mut variable_writes, &execution, child);
                    execution
                }
                super::AdmissionDecision::ReadmitExisting => {
                    let execution = latest_execution(snapshot, &child.task_id)
                        .ok_or(ContainerError::ExecutionNotFound(command.source.clone()))?;
                    // A queued chain successor is admitted by propagation, never
                    // by the fan-out: an execution the seed (the root's own
                    // chain) or a rewind already created stays `Created` until
                    // its predecessor settles. Readmitting it would hand it
                    // `Ready` with no operation to run — and take the admission
                    // the propagation is about to perform.
                    if child.plan == ContainerChildPlan::Queued {
                        resolved.push(execution);
                        continue;
                    }
                    // The latest generation exists but never ran (a Cancel
                    // rewind recreated it): admit it in place instead of
                    // allocating a second live generation.
                    if let Some(change) = readmit_change(snapshot, &execution)? {
                        state_changes.push(change);
                    }
                    if let Some(intent) = child_operation(command, &execution, child.plan)? {
                        operations.push(intent);
                    }
                    child_variables(&mut variable_writes, &execution, child);
                    execution
                }
            };
            resolved.push(execution);
        }

        let plan = TransitionPlan {
            process_id: command.process_id.clone(),
            expected: snapshot.version.clone(),
            execution_creates: creates,
            state_changes,
            variable_writes,
            source_operation: Some(command.source_operation.clone()),
            operations,
            outbox: vec![fanout_outbox(command, "selected")],
        };
        plan.validate()
            .map_err(|error| ContainerError::InvalidPlan(error.to_string()))?;
        Ok(plan)
    }

    fn empty_container_plan(
        snapshot: &AggregateSnapshot,
        command: &ContainerFanoutCommand,
        state: ExecutionState,
        revision: u64,
    ) -> Result<TransitionPlan, ContainerError> {
        let sequence = command
            .source_operation
            .id
            .sequence
            .checked_add(1)
            .ok_or(ContainerError::SequenceOverflow)?;
        let mut operations = Vec::new();
        let propagate_id = OperationId {
            process_id: command.process_id.clone(),
            task_id: command.source.task_id.clone(),
            generation: command.source.generation,
            sequence,
        };
        if !snapshot.operations.contains_key(&propagate_id) {
            operations.push(OperationIntent {
                id: propagate_id,
                kind: OperationKind::PropagateTask,
            });
        }
        let plan = TransitionPlan {
            process_id: command.process_id.clone(),
            expected: snapshot.version.clone(),
            execution_creates: Vec::new(),
            state_changes: vec![StateChange {
                execution: command.source.clone(),
                expected_revision: revision,
                from: state,
                to: ExecutionState::Completed,
            }],
            variable_writes: Vec::new(),
            source_operation: Some(command.source_operation.clone()),
            operations,
            outbox: vec![fanout_outbox(command, "empty")],
        };
        plan.validate()
            .map_err(|error| ContainerError::InvalidPlan(error.to_string()))?;
        Ok(plan)
    }

    /// The join: when a container's children are all terminal (and none was
    /// aborted — an abort owns the container's outcome, exactly like the
    /// legacy `auto_complete`), the parked container completes and hands
    /// propagation to its own chain successor. `None` means "not yet" — or
    /// "not mine": only a `Running` container carrying the park mark joins,
    /// so a child settling under an execution that is still waiting for its
    /// own act decides nothing.
    pub(crate) fn plan_join(
        graph: &ExecutionGraph,
        snapshot: &AggregateSnapshot,
        command: &ContainerJoinCommand,
    ) -> Result<Option<(StateChange, OperationIntent)>, ContainerError> {
        if snapshot.process_id != command.process_id || graph.process_id != command.process_id {
            return Err(ContainerError::WrongAggregate);
        }
        if graph.revision != snapshot.version.graph {
            return Err(ContainerError::GraphVersionConflict {
                graph: graph.revision,
                snapshot: snapshot.version.graph,
            });
        }
        let parent = snapshot
            .executions
            .get(&command.parent)
            .ok_or_else(|| ContainerError::ExecutionNotFound(command.parent.clone()))?;
        // A parent in its own act's flow is not a container: `Waiting` means it
        // owes the client (the legacy `Interrupt`), `Running` means its act is
        // in flight — and a child settling under either must not decide it. The
        // flow fact is what tells a container apart from those; the state alone
        // cannot, since a container is `Running` too.
        if parent.state != ExecutionState::Running {
            return Ok(None);
        }
        if !parked_for_children(snapshot, &command.parent) {
            return Ok(None);
        }
        let children = graph.children(&command.parent);
        if children.is_empty() {
            return Ok(None);
        }
        for child in children {
            let state = snapshot
                .executions
                .get(child)
                .ok_or_else(|| ContainerError::ExecutionNotFound(child.clone()))?
                .state;
            if !state.is_terminal() {
                return Ok(None);
            }
            if state == ExecutionState::Aborted {
                // The abort walk owns this container's outcome; completing it
                // here would double-decide (legacy `auto_complete` refuses).
                return Ok(None);
            }
        }
        let sequence = snapshot
            .operations
            .keys()
            .filter(|id| {
                id.process_id == command.process_id
                    && id.task_id == command.parent.task_id
                    && id.generation == command.parent.generation
            })
            .map(|id| id.sequence)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(ContainerError::SequenceOverflow)?;
        Ok(Some((
            StateChange {
                execution: command.parent.clone(),
                expected_revision: parent.revision,
                from: ExecutionState::Running,
                to: ExecutionState::Completed,
            },
            OperationIntent {
                id: OperationId {
                    process_id: command.process_id.clone(),
                    task_id: command.parent.task_id.clone(),
                    generation: command.parent.generation,
                    sequence,
                },
                kind: OperationKind::PropagateTask,
            },
        )))
    }

    /// The join of a step that is deciding a caught error: every child has
    /// settled and the step itself decides it from its CATCH children, exactly
    /// like the legacy `auto_complete` pass that counted the children of a
    /// failed step.
    ///
    /// The legacy rule is `is_empty_catched`: the step's `Sign::ERROR` was
    /// pinned by its failure hop, `auto_complete` waits for *every* child that
    /// exists (a `timeouts` branch the tick fired included), and then
    ///
    /// - every CATCH child skipped ⇒ no handler matched, so the error is
    ///   re-derived and `emit_error` walks it up: the step is decided
    ///   `Failed` (`CatchJoinOutcome::Resumed`);
    /// - any CATCH child that did not skip ⇒ a handler ran, so the step
    ///   completes and the error is caught (`CatchJoinOutcome::Caught`).
    ///
    /// The join is `None` — "not yet, or not mine" — while a child is still
    /// in flight, when the parent is not deciding its catches (that flow is
    /// what separates a failed step from a container parked for its children),
    /// and when a child was aborted (the abort walk owns that outcome, like the
    /// plain join). Any child's settlement may trigger it: a step that declares
    /// both `catches` and `timeouts` has timeout branches among its children,
    /// and the last one to settle is what completes the gate.
    pub(crate) fn plan_catch_join(
        graph: &ExecutionGraph,
        snapshot: &AggregateSnapshot,
        command: &CatchJoinCommand,
    ) -> Result<Option<CatchJoinOutcome>, ContainerError> {
        if snapshot.process_id != command.process_id || graph.process_id != command.process_id {
            return Err(ContainerError::WrongAggregate);
        }
        if graph.revision != snapshot.version.graph {
            return Err(ContainerError::GraphVersionConflict {
                graph: graph.revision,
                snapshot: snapshot.version.graph,
            });
        }
        // Only an execution whose failure hop pinned the deciding flow decides
        // a caught error, and only one of its own children settling may trigger
        // the decision.
        if !deciding_catches(snapshot, &command.parent) {
            return Ok(None);
        }
        let parent = snapshot
            .executions
            .get(&command.parent)
            .ok_or_else(|| ContainerError::ExecutionNotFound(command.parent.clone()))?;
        // The failed step stays `Running` while its CATCH children run (the
        // legacy `Step::on_error`); a decision that landed meanwhile owns it.
        if parent.state != ExecutionState::Running {
            return Ok(None);
        }
        let children = graph.children(&command.parent);
        if children.is_empty() {
            return Ok(None);
        }
        if !children.contains(&command.source) {
            return Ok(None);
        }
        let mut resumed = true;
        let mut handlers = 0usize;
        for child in children {
            let state = snapshot
                .executions
                .get(child)
                .ok_or_else(|| ContainerError::ExecutionNotFound(child.clone()))?
                .state;
            if !state.is_terminal() {
                return Ok(None);
            }
            if state == ExecutionState::Aborted {
                // The abort walk owns this step's outcome (legacy
                // `auto_complete` refuses to decide an aborted child set).
                return Ok(None);
            }
            // Only the CATCH children decide the caught error — the legacy
            // `is_empty_catched` filters by the sign. The step's other children
            // (a `timeouts` branch the tick fired) gate the join above but do
            // not vote.
            if execution_sign(snapshot, child) != Some(SIGN_CATCH) {
                continue;
            }
            handlers += 1;
            if state != ExecutionState::Skipped {
                resumed = false;
            }
        }
        if handlers == 0 {
            // A decider without handlers is an aggregate the failure fan-out
            // did not produce: refuse to guess a catch outcome for it.
            return Ok(None);
        }
        let sequence =
            snapshot.next_operation_sequence(&command.parent.task_id, command.parent.generation);
        let operation = OperationIntent {
            id: OperationId {
                process_id: command.process_id.clone(),
                task_id: command.parent.task_id.clone(),
                generation: command.parent.generation,
                sequence,
            },
            kind: OperationKind::PropagateTask,
        };
        let change = |to: ExecutionState| StateChange {
            execution: command.parent.clone(),
            expected_revision: parent.revision,
            from: ExecutionState::Running,
            to,
        };
        Ok(Some(if resumed {
            CatchJoinOutcome::Resumed(change(ExecutionState::Failed), operation)
        } else {
            CatchJoinOutcome::Caught(change(ExecutionState::Completed), operation)
        }))
    }
}

/// The existing-generation observation for one task's admission slot: the
/// state of its latest generation. `Created` never ran (`Unstarted`), so a
/// rewound-and-recreated child is readmitted rather than duplicated.
fn existing_generation(snapshot: &AggregateSnapshot, task_id: &str) -> ExistingGeneration {
    let Some(latest) = latest_execution(snapshot, task_id) else {
        return ExistingGeneration::Absent;
    };
    match snapshot.executions[&latest].state {
        ExecutionState::Created => ExistingGeneration::Unstarted,
        ExecutionState::Ready | ExecutionState::Running | ExecutionState::Waiting => {
            ExistingGeneration::Active
        }
        _ => ExistingGeneration::Completed,
    }
}

fn latest_execution(snapshot: &AggregateSnapshot, task_id: &str) -> Option<ExecutionId> {
    snapshot
        .executions
        .keys()
        .filter(|id| id.task_id == task_id)
        .max_by_key(|id| id.generation)
        .cloned()
}

fn next_generation(snapshot: &AggregateSnapshot, task_id: &str) -> Option<u64> {
    snapshot
        .executions
        .keys()
        .filter(|id| id.task_id == task_id)
        .map(|id| id.generation)
        .max()
        .map_or(Some(0), |value| value.checked_add(1))
}

/// Turn symbolic predecessor references into execution ids: the container
/// itself, or a sibling processed earlier in the same command (its resolved
/// id, created or readmitted).
fn resolve_predecessors(
    references: &[ContainerPredecessor],
    source: &ExecutionId,
    resolved: &[ExecutionId],
    current: usize,
) -> Result<Vec<ExecutionId>, ContainerError> {
    references
        .iter()
        .map(|reference| match *reference {
            ContainerPredecessor::Source => Ok(source.clone()),
            ContainerPredecessor::Sibling(index) => {
                if index >= current {
                    return Err(ContainerError::ForwardSibling(index));
                }
                resolved
                    .get(index)
                    .cloned()
                    .ok_or(ContainerError::ForwardSibling(index))
            }
        })
        .collect()
}

/// The admission operation of one child, when its plan needs one.
fn child_operation(
    command: &ContainerFanoutCommand,
    execution: &ExecutionId,
    plan: ContainerChildPlan,
) -> Result<Option<OperationIntent>, ContainerError> {
    let kind = match plan {
        ContainerChildPlan::Queued => return Ok(None),
        ContainerChildPlan::Entry => OperationKind::ExecuteTask,
        ContainerChildPlan::Skip => OperationKind::SkipTask,
    };
    Ok(Some(OperationIntent {
        id: OperationId {
            process_id: command.process_id.clone(),
            task_id: execution.task_id.clone(),
            generation: execution.generation,
            sequence: 0,
        },
        kind,
    }))
}

fn child_variables(
    writes: &mut Vec<VariableWrite>,
    execution: &ExecutionId,
    child: &ContainerChild,
) {
    writes.push(VariableWrite {
        scope: execution.clone(),
        name: "auto".to_string(),
        value: if child.auto { "1" } else { "0" }.to_string(),
    });
    for (name, value) in &child.vars {
        writes.push(VariableWrite {
            scope: execution.clone(),
            name: name.clone(),
            value: value.clone(),
        });
    }
}

/// A readmitted child is `Created`; the admission moves it to `Ready`. An
/// unexpected state is refused rather than forced.
fn readmit_change(
    snapshot: &AggregateSnapshot,
    execution: &ExecutionId,
) -> Result<Option<StateChange>, ContainerError> {
    let existing = snapshot
        .executions
        .get(execution)
        .ok_or_else(|| ContainerError::ExecutionNotFound(execution.clone()))?;
    if existing.state != ExecutionState::Created {
        return Err(ContainerError::ReadmitNotCreated {
            execution: execution.clone(),
            state: existing.state,
        });
    }
    Ok(Some(StateChange {
        execution: execution.clone(),
        expected_revision: existing.revision,
        from: ExecutionState::Created,
        to: ExecutionState::Ready,
    }))
}

fn fanout_outbox(command: &ContainerFanoutCommand, outcome: &str) -> OutboxIntent {
    OutboxIntent {
        key: format!(
            "fanout:{}:{}:{}:{}",
            command.process_id,
            command.source.task_id,
            command.source.generation,
            command.source_operation.id.sequence,
        ),
        topic: format!("fanout.{outcome}"),
        payload: format!("{}:{}", command.source.task_id, command.source.generation),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ContainerError {
    WrongAggregate,
    ExecutionNotFound(ExecutionId),
    SourceNotContainer {
        source: ExecutionId,
        state: ExecutionState,
    },
    SourceOperationTargetsDifferentExecution,
    InvalidSourcePark(ExecutionState),
    EmptyChildTaskId,
    ForwardSibling(usize),
    ReadmitNotCreated {
        execution: ExecutionId,
        state: ExecutionState,
    },
    GraphVersionConflict {
        graph: u64,
        snapshot: u64,
    },
    GenerationOverflow,
    SequenceOverflow,
    InvalidPlan(String),
}
impl fmt::Display for ContainerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "container error: {self:?}")
    }
}
impl std::error::Error for ContainerError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::{AggregateVersion, ClaimToken, ExecutionSnapshot};
    use std::collections::BTreeMap;

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

    #[test]
    fn fanout_keeps_the_source_running_and_admits_entry_and_skip_children() {
        let (snapshot, command) = fanout_command(
            "step1",
            ExecutionState::Running,
            vec![
                child(
                    "b1",
                    ContainerChildPlan::Entry,
                    vec![ContainerPredecessor::Source],
                ),
                child(
                    "b2",
                    ContainerChildPlan::Skip,
                    vec![ContainerPredecessor::Source],
                ),
            ],
        );
        let plan = ContainerPlanner::plan_fanout(&snapshot, &command).unwrap();
        assert_eq!(plan.execution_creates.len(), 2);
        assert!(
            plan.execution_creates
                .iter()
                .all(|create| create.parent == Some(id("step1", 0)))
        );
        let entry = plan
            .execution_creates
            .iter()
            .find(|create| create.execution.task_id == "b1")
            .unwrap();
        assert_eq!(entry.initial_state, ExecutionState::Ready);
        let skip = plan
            .execution_creates
            .iter()
            .find(|create| create.execution.task_id == "b2")
            .unwrap();
        assert_eq!(skip.initial_state, ExecutionState::Ready);
        assert_eq!(plan.operations.len(), 2);
        assert!(
            plan.operations
                .iter()
                .any(|op| op.id.task_id == "b1" && op.kind == OperationKind::ExecuteTask)
        );
        assert!(
            plan.operations
                .iter()
                .any(|op| op.id.task_id == "b2" && op.kind == OperationKind::SkipTask)
        );
        assert_eq!(
            plan.state_changes.len(),
            0,
            "the running source stays running"
        );
        // The auto decision is durable state for the created children.
        assert_eq!(plan.variable_writes.len(), 2);
    }

    /// A container fanning out from `Waiting` — a step whose act the client
    /// just answered — resumes `Running`, exactly like the legacy step whose
    /// `next` runs after its act completed.
    #[test]
    fn fanout_resumes_a_waiting_source_into_running() {
        let (snapshot, command) = fanout_command(
            "step1",
            ExecutionState::Waiting,
            vec![child(
                "b1",
                ContainerChildPlan::Entry,
                vec![ContainerPredecessor::Source],
            )],
        );
        let plan = ContainerPlanner::plan_fanout(&snapshot, &command).unwrap();
        assert_eq!(plan.state_changes.len(), 1);
        let change = &plan.state_changes[0];
        assert_eq!(change.execution, id("step1", 0));
        assert_eq!(change.from, ExecutionState::Waiting);
        assert_eq!(change.to, ExecutionState::Running);
    }

    #[test]
    fn fanout_queues_chain_successors_behind_the_entry() {
        let (snapshot, command) = fanout_command(
            "b1",
            ExecutionState::Running,
            vec![
                child(
                    "s1",
                    ContainerChildPlan::Entry,
                    vec![ContainerPredecessor::Source],
                ),
                child(
                    "s2",
                    ContainerChildPlan::Queued,
                    vec![ContainerPredecessor::Sibling(0)],
                ),
            ],
        );
        let plan = ContainerPlanner::plan_fanout(&snapshot, &command).unwrap();
        assert_eq!(plan.execution_creates.len(), 2);
        let queued = plan
            .execution_creates
            .iter()
            .find(|create| create.execution.task_id == "s2")
            .unwrap();
        assert_eq!(queued.initial_state, ExecutionState::Created);
        assert_eq!(queued.predecessors, vec![id("s1", 0)]);
        // Only the entry receives an operation; propagation admits the rest.
        assert_eq!(plan.operations.len(), 1);
        assert_eq!(plan.operations[0].kind, OperationKind::ExecuteTask);
    }

    #[test]
    fn empty_container_completes_and_propagates_in_the_same_hop() {
        let (snapshot, command) = fanout_command("b1", ExecutionState::Running, vec![]);
        let plan = ContainerPlanner::plan_fanout(&snapshot, &command).unwrap();
        assert!(plan.execution_creates.is_empty());
        assert_eq!(plan.state_changes[0].to, ExecutionState::Completed);
        assert_eq!(plan.operations[0].kind, OperationKind::PropagateTask);
    }

    #[test]
    fn a_rewound_created_child_is_readmitted_not_duplicated() {
        let mut snapshot = snapshot(vec![
            execution("step1", 1, ExecutionState::Running),
            execution("b1", 0, ExecutionState::Cancelled),
            // The Cancel rewind recreated b1 at generation 1; it never ran.
            (
                id("b1", 1),
                ExecutionSnapshot {
                    model_node: node("b1"),
                    parent: Some(id("step1", 1)),
                    predecessors: vec![id("step1", 1)],
                    state: ExecutionState::Created,
                    revision: 0,
                },
            ),
        ]);
        snapshot.version.graph = 2;
        let command = ContainerFanoutCommand {
            process_id: "p1".into(),
            source: id("step1", 1),
            source_operation: OperationCommit {
                id: OperationId {
                    process_id: "p1".into(),
                    task_id: "step1".into(),
                    generation: 1,
                    sequence: 4,
                },
                claim: claim(),
            },
            children: vec![child(
                "b1",
                ContainerChildPlan::Entry,
                vec![ContainerPredecessor::Source],
            )],
            inputs: Vec::new(),
            source_park: ExecutionState::Running,
        };
        let plan = ContainerPlanner::plan_fanout(&snapshot, &command).unwrap();
        assert!(plan.execution_creates.is_empty(), "no duplicate generation");
        // The running source stays running (the park is the state it is
        // already in); the readmitted child is the only change.
        assert_eq!(plan.state_changes.len(), 1);
        assert_eq!(plan.state_changes[0].execution, id("b1", 1));
        assert_eq!(plan.state_changes[0].from, ExecutionState::Created);
        assert_eq!(plan.state_changes[0].to, ExecutionState::Ready);
        assert_eq!(
            plan.operations[0].id,
            OperationId {
                process_id: "p1".into(),
                task_id: "b1".into(),
                generation: 1,
                sequence: 0,
            }
        );
    }

    #[test]
    fn an_active_child_is_ignored_not_doubled() {
        let snapshot = snapshot(vec![
            execution("step1", 0, ExecutionState::Running),
            (
                id("b1", 0),
                ExecutionSnapshot {
                    model_node: node("b1"),
                    parent: Some(id("step1", 0)),
                    predecessors: vec![id("step1", 0)],
                    state: ExecutionState::Running,
                    revision: 1,
                },
            ),
        ]);
        let command = ContainerFanoutCommand {
            process_id: "p1".into(),
            source: id("step1", 0),
            source_operation: commit("step1", 4),
            children: vec![child(
                "b1",
                ContainerChildPlan::Entry,
                vec![ContainerPredecessor::Source],
            )],
            inputs: Vec::new(),
            source_park: ExecutionState::Running,
        };
        let plan = ContainerPlanner::plan_fanout(&snapshot, &command).unwrap();
        assert!(plan.execution_creates.is_empty());
        assert!(plan.operations.is_empty());
    }

    #[test]
    fn a_completed_child_reruns_as_a_fresh_generation() {
        let snapshot = snapshot(vec![
            execution("step1", 1, ExecutionState::Running),
            execution("b1", 0, ExecutionState::Completed),
        ]);
        let command = ContainerFanoutCommand {
            process_id: "p1".into(),
            source: id("step1", 1),
            source_operation: OperationCommit {
                id: OperationId {
                    process_id: "p1".into(),
                    task_id: "step1".into(),
                    generation: 1,
                    sequence: 2,
                },
                claim: claim(),
            },
            children: vec![child(
                "b1",
                ContainerChildPlan::Entry,
                vec![ContainerPredecessor::Source],
            )],
            inputs: Vec::new(),
            source_park: ExecutionState::Running,
        };
        let plan = ContainerPlanner::plan_fanout(&snapshot, &command).unwrap();
        let create = &plan.execution_creates[0];
        assert_eq!(create.execution, id("b1", 1), "next generation allocated");
    }

    #[test]
    fn a_terminal_source_is_not_a_container() {
        let (snapshot, command) = fanout_command(
            "step1",
            ExecutionState::Completed,
            vec![child(
                "b1",
                ContainerChildPlan::Entry,
                vec![ContainerPredecessor::Source],
            )],
        );
        let error = ContainerPlanner::plan_fanout(&snapshot, &command).unwrap_err();
        assert!(matches!(error, ContainerError::SourceNotContainer { .. }));
    }

    /// Mark one execution as a container parked for its children: the flow
    /// the plain join reads (see [`FLOW_CHILDREN`]).
    fn mark_parked(snapshot: &mut AggregateSnapshot, parent: &str) {
        snapshot.variables.insert(
            (id(parent, 0), FLOW_VAR.to_string()),
            FLOW_CHILDREN.to_string(),
        );
    }

    /// The flow fact is one axis with two values, and it is what tells the two
    /// joins apart: a `Running` execution with children decides by its flow and
    /// by nothing else. An execution in its own act's flow carries no flow fact
    /// at all.
    #[test]
    fn the_flow_fact_is_one_axis_with_two_values() {
        let mut snapshot = snapshot(vec![execution("step1", 0, ExecutionState::Running)]);
        let step1 = id("step1", 0);
        assert_eq!(execution_flow(&snapshot, &step1), None);
        assert!(!parked_for_children(&snapshot, &step1));
        assert!(!deciding_catches(&snapshot, &step1));

        snapshot.variables.insert(
            (step1.clone(), FLOW_VAR.to_string()),
            FLOW_CHILDREN.to_string(),
        );
        assert_eq!(
            execution_flow(&snapshot, &step1),
            Some(ExecutionFlow::Children)
        );
        assert!(parked_for_children(&snapshot, &step1));
        assert!(!deciding_catches(&snapshot, &step1));

        snapshot.variables.insert(
            (step1.clone(), FLOW_VAR.to_string()),
            FLOW_CATCHES.to_string(),
        );
        assert_eq!(
            execution_flow(&snapshot, &step1),
            Some(ExecutionFlow::Catches)
        );
        assert!(!parked_for_children(&snapshot, &step1));
        assert!(deciding_catches(&snapshot, &step1));

        // An unknown value is "no flow" rather than a crash: only the two
        // values this module writes are meaningful.
        snapshot
            .variables
            .insert((step1.clone(), FLOW_VAR.to_string()), "?".to_string());
        assert_eq!(execution_flow(&snapshot, &step1), None);
    }

    #[test]
    fn join_completes_the_parent_when_every_child_is_terminal() {
        let mut b = super::super::ExecutionGraphBuilder::new("p1", 1).unwrap();
        for (task, generation) in [("step1", 0u64), ("b1", 0), ("b2", 0)] {
            b.add_node(super::super::ExecutionNode {
                id: id(task, generation),
                model_node: node(task),
                state: ExecutionState::Created,
                revision: 0,
            })
            .unwrap();
        }
        b.set_root(id("step1", 0)).unwrap();
        b.add_child(id("step1", 0), id("b1", 0)).unwrap();
        b.add_child(id("step1", 0), id("b2", 0)).unwrap();
        let graph = b.build().unwrap();
        let mut snapshot = snapshot(vec![
            (
                id("step1", 0),
                ExecutionSnapshot {
                    model_node: node("step1"),
                    parent: None,
                    predecessors: vec![],
                    state: ExecutionState::Running,
                    revision: 3,
                },
            ),
            (
                id("b1", 0),
                ExecutionSnapshot {
                    model_node: node("b1"),
                    parent: Some(id("step1", 0)),
                    predecessors: vec![id("step1", 0)],
                    state: ExecutionState::Completed,
                    revision: 5,
                },
            ),
            (
                id("b2", 0),
                ExecutionSnapshot {
                    model_node: node("b2"),
                    parent: Some(id("step1", 0)),
                    predecessors: vec![id("step1", 0)],
                    state: ExecutionState::Skipped,
                    revision: 4,
                },
            ),
        ]);
        mark_parked(&mut snapshot, "step1");
        let command = ContainerJoinCommand {
            process_id: "p1".into(),
            parent: id("step1", 0),
        };
        let (change, operation) = ContainerPlanner::plan_join(&graph, &snapshot, &command)
            .unwrap()
            .unwrap();
        assert_eq!(change.execution, id("step1", 0));
        assert_eq!(change.from, ExecutionState::Running);
        assert_eq!(change.to, ExecutionState::Completed);
        assert_eq!(operation.kind, OperationKind::PropagateTask);
        assert_eq!(operation.id.task_id, "step1");
        assert_eq!(operation.id.sequence, 1);
    }

    #[test]
    fn join_waits_for_running_children_and_never_fires_on_an_aborted_one() {
        let graph = {
            let mut b = super::super::ExecutionGraphBuilder::new("p1", 1).unwrap();
            for task in ["step1", "b1", "b2"] {
                b.add_node(super::super::ExecutionNode {
                    id: id(task, 0),
                    model_node: node(task),
                    state: ExecutionState::Created,
                    revision: 0,
                })
                .unwrap();
            }
            b.set_root(id("step1", 0)).unwrap();
            b.add_child(id("step1", 0), id("b1", 0)).unwrap();
            b.add_child(id("step1", 0), id("b2", 0)).unwrap();
            b.build().unwrap()
        };
        let command = ContainerJoinCommand {
            process_id: "p1".into(),
            parent: id("step1", 0),
        };
        // The parent is a running container carrying the park mark: that is
        // the state a child's settlement decides.
        let parked_child = |b1: ExecutionState, b2: ExecutionState| {
            let mut snapshot = snapshot(vec![
                (
                    id("step1", 0),
                    ExecutionSnapshot {
                        model_node: node("step1"),
                        parent: None,
                        predecessors: vec![],
                        state: ExecutionState::Running,
                        revision: 3,
                    },
                ),
                (
                    id("b1", 0),
                    ExecutionSnapshot {
                        model_node: node("b1"),
                        parent: Some(id("step1", 0)),
                        predecessors: vec![id("step1", 0)],
                        state: b1,
                        revision: 1,
                    },
                ),
                (
                    id("b2", 0),
                    ExecutionSnapshot {
                        model_node: node("b2"),
                        parent: Some(id("step1", 0)),
                        predecessors: vec![id("step1", 0)],
                        state: b2,
                        revision: 1,
                    },
                ),
            ]);
            mark_parked(&mut snapshot, "step1");
            snapshot
        };
        assert!(
            ContainerPlanner::plan_join(
                &graph,
                &parked_child(ExecutionState::Running, ExecutionState::Completed),
                &command
            )
            .unwrap()
            .is_none()
        );
        // An aborted child leaves the outcome to the abort walk.
        assert!(
            ContainerPlanner::plan_join(
                &graph,
                &parked_child(ExecutionState::Completed, ExecutionState::Aborted),
                &command
            )
            .unwrap()
            .is_none()
        );
        // A settled child set under a parent nobody marked decides nothing:
        // an execution waiting for its own act (the legacy `Interrupt`) is
        // `Waiting`, and one whose act is in flight is `Running` — neither is
        // a container until its flow says so.
        let mut unmarked = parked_child(ExecutionState::Completed, ExecutionState::Skipped);
        unmarked.variables.clear();
        assert!(
            ContainerPlanner::plan_join(&graph, &unmarked, &command)
                .unwrap()
                .is_none()
        );
        // A parent deciding its catches is never joined by the plain join
        // either — that flow belongs to the catch join.
        let mut decider = parked_child(ExecutionState::Completed, ExecutionState::Skipped);
        decider.variables.insert(
            (id("step1", 0), FLOW_VAR.to_string()),
            FLOW_CATCHES.to_string(),
        );
        assert!(
            ContainerPlanner::plan_join(&graph, &decider, &command)
                .unwrap()
                .is_none()
        );
        // A `Waiting` parent is never joined either: its act is unanswered.
        let mut waiting = parked_child(ExecutionState::Completed, ExecutionState::Skipped);
        waiting.executions.insert(
            id("step1", 0),
            ExecutionSnapshot {
                model_node: node("step1"),
                parent: None,
                predecessors: vec![],
                state: ExecutionState::Waiting,
                revision: 3,
            },
        );
        assert!(
            ContainerPlanner::plan_join(&graph, &waiting, &command)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_failure_fanout_keeps_the_step_running() {
        let (snapshot, mut command) = fanout_command(
            "step1",
            ExecutionState::Waiting,
            vec![child(
                "c1",
                ContainerChildPlan::Entry,
                vec![ContainerPredecessor::Source],
            )],
        );
        command.source_park = ExecutionState::Running;
        let plan = ContainerPlanner::plan_fanout(&snapshot, &command).unwrap();
        let change = plan
            .state_changes
            .iter()
            .find(|change| change.execution == id("step1", 0))
            .expect("the waiting step moves to Running");
        assert_eq!(change.from, ExecutionState::Waiting);
        assert_eq!(change.to, ExecutionState::Running);
        assert!(plan.state_changes.iter().all(|change| {
            change.to != ExecutionState::Waiting && change.to != ExecutionState::Completed
        }));

        // An already-running step keeps its state: the fan-out adds no change.
        let (snapshot, mut command) = fanout_command(
            "step1",
            ExecutionState::Running,
            vec![child(
                "c1",
                ContainerChildPlan::Entry,
                vec![ContainerPredecessor::Source],
            )],
        );
        command.source_park = ExecutionState::Running;
        let plan = ContainerPlanner::plan_fanout(&snapshot, &command).unwrap();
        assert!(
            plan.state_changes
                .iter()
                .all(|change| change.execution != id("step1", 0)),
            "a running source has no state change to park it"
        );
    }

    fn catch_graph() -> ExecutionGraph {
        let mut b = super::super::ExecutionGraphBuilder::new("p1", 1).unwrap();
        for task in ["step1", "c1", "c2"] {
            b.add_node(super::super::ExecutionNode {
                id: id(task, 0),
                model_node: node(task),
                state: ExecutionState::Created,
                revision: 0,
            })
            .unwrap();
        }
        b.set_root(id("step1", 0)).unwrap();
        b.add_child(id("step1", 0), id("c1", 0)).unwrap();
        b.add_child(id("step1", 0), id("c2", 0)).unwrap();
        b.build().unwrap()
    }

    /// A failed step deciding its caught error: the step carries the ERROR
    /// sign, its CATCH children carry theirs, and the children's states are
    /// the argument.
    fn catch_snapshot(c1: ExecutionState, c2: ExecutionState) -> AggregateSnapshot {
        let mut snapshot = snapshot(vec![
            (
                id("step1", 0),
                ExecutionSnapshot {
                    model_node: node("step1"),
                    parent: None,
                    predecessors: vec![],
                    state: ExecutionState::Running,
                    revision: 3,
                },
            ),
            (
                id("c1", 0),
                ExecutionSnapshot {
                    model_node: node("c1"),
                    parent: Some(id("step1", 0)),
                    predecessors: vec![id("step1", 0)],
                    state: c1,
                    revision: 1,
                },
            ),
            (
                id("c2", 0),
                ExecutionSnapshot {
                    model_node: node("c2"),
                    parent: Some(id("step1", 0)),
                    predecessors: vec![id("step1", 0)],
                    state: c2,
                    revision: 1,
                },
            ),
        ]);
        // The failed step carries the deciding flow; its children carry the
        // handler role.
        snapshot.variables.insert(
            (id("step1", 0), FLOW_VAR.to_string()),
            FLOW_CATCHES.to_string(),
        );
        for task in ["c1", "c2"] {
            snapshot
                .variables
                .insert((id(task, 0), SIGN_VAR.to_string()), SIGN_CATCH.to_string());
        }
        snapshot
    }

    fn catch_command() -> CatchJoinCommand {
        CatchJoinCommand {
            process_id: "p1".into(),
            parent: id("step1", 0),
            source: id("c1", 0),
        }
    }

    #[test]
    fn catch_join_resumes_the_error_when_every_catcher_skipped() {
        let graph = catch_graph();
        let snapshot = catch_snapshot(ExecutionState::Skipped, ExecutionState::Skipped);
        let outcome =
            ContainerPlanner::plan_catch_join(&graph, &snapshot, &catch_command()).unwrap();
        let Some(CatchJoinOutcome::Resumed(change, operation)) = outcome else {
            panic!("every skipped catcher resumes the error");
        };
        assert_eq!(change.execution, id("step1", 0));
        assert_eq!(change.from, ExecutionState::Running);
        assert_eq!(change.to, ExecutionState::Failed);
        assert_eq!(operation.kind, OperationKind::PropagateTask);
        assert_eq!(operation.id.task_id, "step1");
        assert_eq!(operation.id.sequence, 0);
    }

    #[test]
    fn catch_join_catches_when_a_catcher_did_not_skip() {
        let graph = catch_graph();
        for (c1, c2) in [
            (ExecutionState::Completed, ExecutionState::Skipped),
            (ExecutionState::Completed, ExecutionState::Completed),
            // The legacy `is_empty_catched` only counts skips, so a failed
            // handler also leaves the error caught.
            (ExecutionState::Failed, ExecutionState::Skipped),
        ] {
            let snapshot = catch_snapshot(c1, c2);
            let outcome =
                ContainerPlanner::plan_catch_join(&graph, &snapshot, &catch_command()).unwrap();
            let Some(CatchJoinOutcome::Caught(change, operation)) = outcome else {
                panic!("a non-skipped catcher catches the error ({c1:?}/{c2:?})");
            };
            assert_eq!(change.from, ExecutionState::Running);
            assert_eq!(change.to, ExecutionState::Completed);
            assert_eq!(operation.kind, OperationKind::PropagateTask);
        }
    }

    #[test]
    fn catch_join_waits_for_an_in_flight_catcher_and_yields_to_the_abort_walk() {
        let graph = catch_graph();
        for (c1, c2) in [
            (ExecutionState::Running, ExecutionState::Completed),
            (ExecutionState::Created, ExecutionState::Skipped),
            (ExecutionState::Completed, ExecutionState::Aborted),
        ] {
            let snapshot = catch_snapshot(c1, c2);
            assert!(
                ContainerPlanner::plan_catch_join(&graph, &snapshot, &catch_command())
                    .unwrap()
                    .is_none(),
                "{c1:?}/{c2:?} must not decide the step"
            );
        }
    }

    /// A step that declares both `catches` and `timeouts` has a *mixed* child
    /// set: the handlers decide the caught error, a timeout branch the tick
    /// fired only gates the join (the legacy `auto_complete` waits for every
    /// child, `is_empty_catched` counts the CATCH ones), and any child's
    /// settlement — the timeout branch's included — may trigger the decision.
    #[test]
    fn catch_join_decides_a_mixed_child_set_from_its_handlers() {
        let graph = {
            let mut b = super::super::ExecutionGraphBuilder::new("p1", 1).unwrap();
            for task in ["step1", "c1", "t1"] {
                b.add_node(super::super::ExecutionNode {
                    id: id(task, 0),
                    model_node: node(task),
                    state: ExecutionState::Created,
                    revision: 0,
                })
                .unwrap();
            }
            b.set_root(id("step1", 0)).unwrap();
            b.add_child(id("step1", 0), id("c1", 0)).unwrap();
            b.add_child(id("step1", 0), id("t1", 0)).unwrap();
            b.build().unwrap()
        };
        let mixed = |handler: ExecutionState, timeout: ExecutionState| {
            let mut snapshot = snapshot(vec![
                (
                    id("step1", 0),
                    ExecutionSnapshot {
                        model_node: node("step1"),
                        parent: None,
                        predecessors: vec![],
                        state: ExecutionState::Running,
                        revision: 3,
                    },
                ),
                (
                    id("c1", 0),
                    ExecutionSnapshot {
                        model_node: node("c1"),
                        parent: Some(id("step1", 0)),
                        predecessors: vec![id("step1", 0)],
                        state: handler,
                        revision: 1,
                    },
                ),
                (
                    id("t1", 0),
                    ExecutionSnapshot {
                        model_node: node("t1"),
                        parent: Some(id("step1", 0)),
                        predecessors: vec![id("step1", 0)],
                        state: timeout,
                        revision: 1,
                    },
                ),
            ]);
            snapshot.variables.insert(
                (id("step1", 0), FLOW_VAR.to_string()),
                FLOW_CATCHES.to_string(),
            );
            snapshot
                .variables
                .insert((id("c1", 0), SIGN_VAR.to_string()), SIGN_CATCH.to_string());
            snapshot
        };
        // Triggered by the timeout branch's settlement, not the handler's: the
        // handler caught the error and the branch has settled.
        let caught = ContainerPlanner::plan_catch_join(
            &graph,
            &mixed(ExecutionState::Completed, ExecutionState::Completed),
            &CatchJoinCommand {
                process_id: "p1".into(),
                parent: id("step1", 0),
                source: id("t1", 0),
            },
        )
        .unwrap()
        .expect("the timeout branch's settlement completes the gate");
        assert!(matches!(caught, CatchJoinOutcome::Caught(..)));
        // The handler skipped instead: the error resumes, still triggered by
        // the timeout branch.
        let resumed = ContainerPlanner::plan_catch_join(
            &graph,
            &mixed(ExecutionState::Skipped, ExecutionState::Completed),
            &CatchJoinCommand {
                process_id: "p1".into(),
                parent: id("step1", 0),
                source: id("t1", 0),
            },
        )
        .unwrap()
        .expect("the timeout branch's settlement completes the gate");
        assert!(matches!(resumed, CatchJoinOutcome::Resumed(..)));
        // The timeout branch is still in flight: the gate holds, however the
        // handler settled.
        for handler in [ExecutionState::Completed, ExecutionState::Skipped] {
            assert!(
                ContainerPlanner::plan_catch_join(
                    &graph,
                    &mixed(handler, ExecutionState::Waiting),
                    &CatchJoinCommand {
                        process_id: "p1".into(),
                        parent: id("step1", 0),
                        source: id("t1", 0),
                    },
                )
                .unwrap()
                .is_none(),
                "a live timeout branch gates the decision (handler {handler:?})"
            );
        }
        // A foreign execution's settlement triggers nothing.
        assert!(
            ContainerPlanner::plan_catch_join(
                &graph,
                &mixed(ExecutionState::Completed, ExecutionState::Completed),
                &CatchJoinCommand {
                    process_id: "p1".into(),
                    parent: id("step1", 0),
                    source: id("elsewhere", 0),
                },
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn catch_join_only_fires_for_a_flow_decided_step_and_its_catcher() {
        let graph = catch_graph();
        let settled = || catch_snapshot(ExecutionState::Skipped, ExecutionState::Skipped);

        // A step that never failed (no deciding flow) is not deciding anything.
        let mut snapshot = settled();
        snapshot
            .variables
            .remove(&(id("step1", 0), FLOW_VAR.to_string()));
        assert!(
            ContainerPlanner::plan_catch_join(&graph, &snapshot, &catch_command())
                .unwrap()
                .is_none()
        );

        // A step parked for children is a plain container, not a decider: the
        // catch join must not decide it either.
        let mut snapshot = settled();
        snapshot.variables.insert(
            (id("step1", 0), FLOW_VAR.to_string()),
            FLOW_CHILDREN.to_string(),
        );
        assert!(
            ContainerPlanner::plan_catch_join(&graph, &snapshot, &catch_command())
                .unwrap()
                .is_none()
        );

        // The triggering child must belong to the deciding step, but its role
        // no longer has to be a handler one: a step declaring `timeouts` beside
        // its `catches` is decided by whichever child settles last.
        let mut snapshot = settled();
        snapshot
            .variables
            .remove(&(id("c1", 0), SIGN_VAR.to_string()));
        assert!(
            ContainerPlanner::plan_catch_join(&graph, &snapshot, &catch_command())
                .unwrap()
                .is_some(),
            "the handler role must not gate the trigger"
        );

        // A child without the role does not vote: the decision still comes from
        // the handler (c1 skipped, so the error resumes).
        let mut snapshot = settled();
        snapshot
            .variables
            .remove(&(id("c2", 0), SIGN_VAR.to_string()));
        let outcome = ContainerPlanner::plan_catch_join(&graph, &snapshot, &catch_command())
            .unwrap()
            .expect("the signed handler still decides");
        assert!(matches!(outcome, CatchJoinOutcome::Resumed(..)));

        // A decider with no handler child at all is an aggregate the failure
        // fan-out did not produce: refuse to guess a catch outcome for it.
        let mut snapshot = settled();
        for task in ["c1", "c2"] {
            snapshot
                .variables
                .remove(&(id(task, 0), SIGN_VAR.to_string()));
        }
        assert!(
            ContainerPlanner::plan_catch_join(&graph, &snapshot, &catch_command())
                .unwrap()
                .is_none()
        );
    }
}
