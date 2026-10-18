//! Folding the pure-internal operations into the plan that produced them.
//!
//! [`PropagateTask`], [`SkipTask`], [`AbortTask`] and [`ReenterTask`] run
//! nothing of their own: phase 2 is the snapshot, and their whole work is the
//! plan their creator's commit already implied. Creating them as durable
//! operations therefore buys a claim, a load, a plan and a second commit for a
//! transition the creator can compute inside its own atomic batch. This module
//! does exactly that, and only that: before the creator commits, every
//! pure-internal intent the plan (or a folded plan, transitively) creates is
//! re-planned from a *scratch* snapshot that applies the pending deltas, its
//! plan is merged into the creator's, and the intent itself is never committed.
//!
//! The scratch snapshot keeps the folded intents as placeholders. That is what
//! preserves identity: `next_operation_sequence`, the operation ids the folded
//! plans create, and the `propagate:`/`skipped:`/`reenter:` outbox keys keep
//! the exact ids and keys they had when each pure operation committed on its
//! own. Only their durable records disappear.
//!
//! What remains durable is the external boundary: [`ExecuteTask`] (guard
//! evaluation, package resolution, the start of a new execution),
//! [`ApplyAction`] (the replay of a client decision or a package effect) and
//! [`DispatchTimeout`] (a clock wait). Their intents stay in the folded plan.
//!
//! A folded plan is one commit: either the effect/completion and everything the
//! internal cascade decided lands together, or — when the merge cannot be
//! planned — the caller commits the original plan and the pure operations stay
//! durable, exactly as they were before the fold. A lost compare-and-swap
//! therefore still hands an effect-free operation back to recovery, and an
//! operation that crossed the effect boundary still goes `EffectUnknown`
//! (A1a).

use super::*;

use crate::scheduler::core::{
    commit::aggregate::{ProcessSnapshot, TaskSnapshot},
    model::{
        operation::{ClaimToken, OperationId, OperationKind, OperationRecord},
        task_graph::TaskId,
        transition_plan::{
            OperationCommit, OperationIntent, StateChange, TransitionPlan, VariableWrite,
        },
    },
};

use std::collections::{HashMap, VecDeque};

/// The kinds whose entire body is a plan derived from the state their creator
/// just committed. They are folded into that creator's commit instead of
/// becoming durable operations of their own.
///
/// The persisted vocabulary stays: an aggregate written by an earlier kernel
/// may still carry one of these operations, and the host's own driver is what
/// folds the new intents (and what a legacy operation still uses to reach its
/// next plan).
pub(super) fn folds_into_plan(kind: OperationKind) -> bool {
    matches!(
        kind,
        OperationKind::PropagateTask
            | OperationKind::SkipTask
            | OperationKind::AbortTask
            | OperationKind::ReenterTask
    )
}

impl KernelHost {
    /// Run every pure-internal intent this plan creates to its own plan, in the
    /// order the durable operations would have been woken, and merge the
    /// results into one atomic transition.
    ///
    /// The scratch snapshot is the aggregate at the claim plus this plan's
    /// pending deltas. Every folded operation sees — and contributes to — that
    /// same view, so a cascade that settles a task created by the plan itself
    /// (an unselected branch) is decided without inventing an intermediate
    /// commit. Tasks a folded plan creates are settled in the create itself
    /// when the cascade decides them in the same batch; no plan ever carries a
    /// state change for a task it also creates.
    pub(in crate::scheduler::shell) async fn fold_internal(
        &self,
        snapshot: &ProcessSnapshot,
        plan: TransitionPlan,
        now_ms: i64,
    ) -> Result<TransitionPlan> {
        // The common operation (a start, a package effect, a timer round)
        // creates no hand-off at all: never pay for the scratch snapshot.
        if !plan
            .operations
            .iter()
            .any(|intent| folds_into_plan(intent.kind))
        {
            return Ok(plan);
        }
        let mut scratch = snapshot.clone();
        apply_pending(&mut scratch, &plan);

        let mut folded = TransitionPlan {
            process_id: plan.process_id.clone(),
            expected: plan.expected.clone(),
            task_creates: Vec::new(),
            state_changes: Vec::new(),
            variable_writes: Vec::new(),
            // The folded plan is still the claimed operation's own transition.
            source_operation: plan.source_operation.clone(),
            operations: Vec::new(),
            outbox: Vec::new(),
        };
        // The tasks this batch creates, by id: a cascade that decides one of
        // them writes its terminal state into the create, never a state change
        // the commit would reject against the pre-batch aggregate.
        let mut created: HashMap<TaskId, usize> = HashMap::with_capacity(plan.task_creates.len());
        for create in plan.task_creates {
            created.insert(create.task.clone(), folded.task_creates.len());
            folded.task_creates.push(create);
        }
        for change in plan.state_changes {
            merge_change(&mut folded, &created, change)?;
        }
        for write in plan.variable_writes {
            merge_write(&mut folded, write);
        }
        let mut queue: VecDeque<OperationIntent> = VecDeque::new();
        for intent in plan.operations {
            if folds_into_plan(intent.kind) {
                queue.push_back(intent);
            } else {
                folded.operations.push(intent);
            }
        }
        folded.outbox.extend(plan.outbox);

        let mut planned: HashMap<OperationId, OperationKind> = HashMap::new();
        while let Some(intent) = queue.pop_front() {
            if let Some(previous) = planned.insert(intent.id.clone(), intent.kind) {
                if previous == intent.kind {
                    // A cascade that re-derives the same hand-off twice (an
                    // idempotent propagation over two settled predecessors) is
                    // one durable transition; planning it twice would
                    // duplicate its outbox record.
                    continue;
                }
                return Err(runtime(format!(
                    "folded plan re-creates operation {:?} as {:?} after {:?}",
                    intent.id, intent.kind, previous
                )));
            }
            let driver = driver(intent.kind).ok_or_else(|| {
                runtime(format!(
                    "operation kind {:?} has no planning driver",
                    intent.kind
                ))
            })?;
            let id = intent.id;
            // The claim names the operation the intent *would* have been: the
            // folded plan's outbox keys are derived from its identity, and
            // they must not move.
            let claim = OperationCommit {
                id: id.clone(),
                claim: ClaimToken {
                    owner: "fold".into(),
                    fence: 1,
                    lease_until_ms: i64::MAX,
                    heartbeat_revision: 0,
                },
            };
            let local = {
                let task_id = id.task.clone();
                // A4: the scratch view plans inside the producing command's
                // own lane (the fold never runs an effect), so the synthetic
                // operation carries a detached lane of its own.
                let mut detached = CommandLane::detached(self);
                let operation = Operation {
                    id: &id,
                    claim: &claim,
                    task_id: task_id.clone(),
                    task: scratch.tasks.get(&task_id),
                    snapshot: &scratch,
                    now_ms,
                    lane: &mut detached,
                };
                driver.next(self, &operation, Outcome::Planned).await?
            };
            // The local plan may itself hand a decision to another folded
            // kind (a propagation's join, a dead successor's cascade). Apply
            // it to the scratch view first so the next plan reads the state
            // the durable sequence would have read.
            apply_pending(&mut scratch, &local);
            for create in local.task_creates {
                if created
                    .insert(create.task.clone(), folded.task_creates.len())
                    .is_some()
                {
                    return Err(runtime(format!(
                        "folded plan creates execution {:?} twice",
                        create.task
                    )));
                }
                folded.task_creates.push(create);
            }
            for change in local.state_changes {
                merge_change(&mut folded, &created, change)?;
            }
            for write in local.variable_writes {
                merge_write(&mut folded, write);
            }
            for intent in local.operations {
                if folds_into_plan(intent.kind) {
                    queue.push_back(intent);
                } else {
                    folded.operations.push(intent);
                }
            }
            folded.outbox.extend(local.outbox);
        }
        folded.validate().map_err(runtime)?;
        Ok(folded)
    }
}

/// Apply a plan's value changes to a reading snapshot. Revisions and the
/// process counters are left alone: every plan merged into the folded batch
/// is formed against the aggregate at the claim, so the generated state
/// changes carry the pre-batch revision the commit checks.
fn apply_pending(snapshot: &mut ProcessSnapshot, plan: &TransitionPlan) {
    for create in &plan.task_creates {
        snapshot.tasks.insert(
            create.task.clone(),
            TaskSnapshot {
                model_node: create.model_node.clone(),
                parent: create.parent.clone(),
                predecessors: create.predecessors.clone(),
                state: create.initial_state,
                revision: 0,
            },
        );
    }
    for change in &plan.state_changes {
        if let Some(task) = snapshot.tasks.get_mut(&change.task) {
            task.state = change.to;
        }
    }
    for write in &plan.variable_writes {
        snapshot.variables.insert(
            (write.scope.clone(), write.name.clone()),
            write.value.clone(),
        );
    }
    for intent in &plan.operations {
        snapshot
            .operations
            .entry(intent.id.clone())
            .or_insert_with(|| OperationRecord::pending(intent.id.clone(), intent.kind));
    }
}

/// Merge one state change into the folded batch.
///
/// A change of a task the batch itself creates becomes that create's initial
/// state: the aggregate never observes the intermediate state (the pure
/// operation that would have written it no longer exists), and the commit
/// cannot apply a state change to a task it creates. A change of a task an
/// earlier merged plan already moved keeps the batch's single change, with the
/// later target — exactly the last-writer state the ordered commits left.
fn merge_change(
    folded: &mut TransitionPlan,
    created: &HashMap<TaskId, usize>,
    change: StateChange,
) -> Result<()> {
    if let Some(index) = created.get(&change.task) {
        if !change.to.is_terminal() {
            return Err(runtime(format!(
                "folded plan settled created execution {:?} into non-terminal {:?}",
                change.task, change.to
            )));
        }
        folded.task_creates[*index].initial_state = change.to;
        folded
            .state_changes
            .retain(|existing| existing.task != change.task);
        // A start the same batch created for an execution the cascade has
        // already decided must not stay in the ready queue: it would announce
        // an execution that never ran (and would find nothing startable).
        folded
            .operations
            .retain(|intent| intent.id.task != change.task);
        return Ok(());
    }
    if let Some(existing) = folded
        .state_changes
        .iter_mut()
        .find(|existing| existing.task == change.task)
    {
        existing.to = change.to;
        return Ok(());
    }
    folded.state_changes.push(change);
    Ok(())
}

/// Merge one variable write: the later plan wins, at the position the first
/// write took (the commit applies the batch in order and the aggregate keeps
/// one value per scope/name).
fn merge_write(folded: &mut TransitionPlan, write: VariableWrite) {
    if let Some(existing) = folded
        .variable_writes
        .iter_mut()
        .find(|existing| existing.scope == write.scope && existing.name == write.name)
    {
        existing.value = write.value;
        return;
    }
    folded.variable_writes.push(write);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::core::model::task_graph::TaskState;
    use crate::scheduler::core::model::transition_plan::{ProcessVersion, TaskCreate};

    fn version() -> ProcessVersion {
        ProcessVersion {
            graph: 1,
            process: 2,
            operations: 3,
        }
    }

    fn task(node: &str, generation: u64) -> TaskId {
        TaskId::new(node, generation).unwrap()
    }

    fn create(node: &str) -> TaskCreate {
        TaskCreate {
            task: task(node, 0),
            model_node: crate::scheduler::core::model::model_graph::NodeId::parse(node).unwrap(),
            parent: None,
            predecessors: Vec::new(),
            initial_state: TaskState::Ready,
        }
    }

    /// A cascade that decides a task the same batch created writes the terminal
    /// state into the create instead of a state change the commit would refuse.
    #[test]
    fn a_created_execution_takes_the_cascade_state_as_its_initial_state() {
        let mut folded = TransitionPlan {
            process_id: "p".into(),
            expected: version(),
            task_creates: vec![create("b2")],
            state_changes: Vec::new(),
            variable_writes: Vec::new(),
            source_operation: None,
            operations: Vec::new(),
            outbox: Vec::new(),
        };
        let mut created = HashMap::new();
        created.insert(task("b2", 0), 0);
        merge_change(
            &mut folded,
            &created,
            StateChange {
                task: task("b2", 0),
                expected_revision: 0,
                from: TaskState::Ready,
                to: TaskState::Skipped,
            },
        )
        .unwrap();
        assert_eq!(folded.task_creates[0].initial_state, TaskState::Skipped);
        assert!(folded.state_changes.is_empty());
    }

    /// Two settled predecessors may hand the same target the same skip; the
    /// batch keeps one change, not a duplicate the commit refuses.
    #[test]
    fn a_repeated_state_change_keeps_the_later_target() {
        let mut folded = TransitionPlan {
            process_id: "p".into(),
            expected: version(),
            task_creates: Vec::new(),
            state_changes: vec![StateChange {
                task: task("a", 0),
                expected_revision: 3,
                from: TaskState::Running,
                to: TaskState::Completed,
            }],
            variable_writes: Vec::new(),
            source_operation: None,
            operations: Vec::new(),
            outbox: Vec::new(),
        };
        merge_change(
            &mut folded,
            &HashMap::new(),
            StateChange {
                task: task("a", 0),
                expected_revision: 3,
                from: TaskState::Completed,
                to: TaskState::Failed,
            },
        )
        .unwrap();
        assert_eq!(folded.state_changes.len(), 1);
        assert_eq!(folded.state_changes[0].from, TaskState::Running);
        assert_eq!(folded.state_changes[0].to, TaskState::Failed);
    }
}
