//! The `ReturnToParent` operation: a subflow child's hand-back.
//!
//! A subflow act starts a run of its own and then waits. The child's terminal
//! commit creates this operation in the child aggregate, so the hand-back is
//! durable exactly like the child's outcome: a crash between the child's
//! terminal commit and the parent's decision leaves the operation pending and
//! the child aggregate retained, and the ready/recovery paths re-drive it.
//!
//! The operation maps the child's terminal state onto the parent act's action
//! (the legacy `Runtime::return_to_act`): outputs ride the action as inputs,
//! `Completed` completes the act, `Errored` fails it with the child's durable
//! error, `Aborted`/`Skipped` abort/skip it. It only decides while the parent
//! act is `Waiting` — the state the start operation committed *before* the
//! effect started the child — so the child can never race the parent's own
//! effect commit. A parent that is already terminal means the decision was
//! already delivered (or the parent was decided elsewhere); the operation then
//! retires, which is what makes a re-driven hand-back exactly-once.

use super::*;
use crate::scheduler::core::commit::command::ActionKind;
use crate::scheduler::core::decision::run_outcome;
use crate::scheduler::core::model::operation::OperationState;
use crate::scheduler::core::model::task_graph::TaskState;
use crate::utils::consts;

/// How long the pre-claim check waits for a parent that is still finishing its
/// own effect commit (the resolution-failure path starts `Running`). Normal
/// subflows are already `Waiting` before the child exists, so this only covers
/// the rare state; the poll net remains the fallback.
const PARENT_WAIT_ROUNDS: usize = 50;
const PARENT_WAIT_MS: u64 = 10;

/// The durable parent linkage a subflow child carries on its root scope.
struct ParentLink {
    pid: String,
    tid: String,
}

/// The parent act's readiness for the hand-back.
enum ParentState {
    /// The act is waiting for the child: the decision may be applied.
    Waiting,
    /// The act cannot be decided (already terminal, or its run is gone): the
    /// hand-back retires.
    Done,
    /// The parent is still finishing its own effect commit.
    Pending,
}

/// The child's parent linkage, from the root-scope facts the subflow package
/// wrote into its start inputs.
fn parent_link(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
) -> Result<Option<ParentLink>> {
    let Some(root) = root_task(snapshot) else {
        return Ok(None);
    };
    let read = |name: &str| {
        snapshot
            .variables
            .get(&(root.clone(), name.to_string()))
            .map(|raw| json_text(raw))
    };
    match (
        read(consts::ACT_USE_PARENT_PROC_ID),
        read(consts::ACT_USE_PARENT_TASK_ID),
    ) {
        (Some(pid), Some(tid)) if !pid.is_empty() && !tid.is_empty() => {
            Ok(Some(ParentLink { pid, tid }))
        }
        _ => Ok(None),
    }
}

/// The parent act's state, by the instance wire the child carries.
async fn parent_state(host: &KernelHost, link: &ParentLink) -> Result<ParentState> {
    let Some(loaded) = host.load(&link.pid).await? else {
        return Ok(ParentState::Done);
    };
    let task = loaded
        .snapshot
        .tasks
        .iter()
        .find(|(id, _)| id.wire(&link.pid) == link.tid)
        .map(|(_, task)| task.state);
    match task {
        None => Ok(ParentState::Done),
        Some(state) if state.is_terminal() => Ok(ParentState::Done),
        Some(TaskState::Waiting) => {
            // The act is `Waiting` from the start commit *before* its effect
            // started the child. The return must not complete it while that
            // effect operation is still in flight — the parent's own commit
            // would then lose its guard and turn the already-run effect into
            // `EffectUnknown`. Wait for the effect operation to retire.
            let effects: Vec<_> = loaded
                .snapshot
                .operations
                .iter()
                .filter(|(id, operation)| {
                    id.task.wire(&link.pid) == link.tid
                        && operation.kind == OperationKind::ApplyAction
                })
                .map(|(_, operation)| operation.state.clone())
                .collect();
            if effects.is_empty()
                || effects
                    .iter()
                    .all(|state| matches!(state, OperationState::Completed(_)))
            {
                Ok(ParentState::Waiting)
            } else {
                Ok(ParentState::Pending)
            }
        }
        Some(_) => Ok(ParentState::Pending),
    }
}

/// The action the child's terminal outcome means for the parent act.
fn return_action(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
) -> Result<ActionKind> {
    let root = root_task(snapshot).ok_or_else(|| runtime("a settled run has no root execution"))?;
    Ok(match run_outcome(snapshot) {
        Some(crate::RunOutcome::Errored { code, message }) => ActionKind::Fail { code, message },
        Some(crate::RunOutcome::Terminated) => {
            match snapshot.tasks.get(&root).map(|task| task.state) {
                Some(TaskState::Aborted) => ActionKind::Abort,
                Some(TaskState::Skipped | TaskState::Removed) => ActionKind::Skip,
                // The legacy `return_to_act` mapped every other terminal state to
                // `Next` (the cancelled/removed vocabulary had no hand-back
                // action of its own).
                _ => ActionKind::Complete,
            }
        }
        _ => ActionKind::Complete,
    })
}

/// The child's outputs, as the parent action's inputs (the legacy hand-back
/// folded `proc.outputs()` into the parent act). The linkage facts and the
/// failure vocabulary are engine state, never workflow data.
fn return_inputs(
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
) -> Result<Vec<(String, String)>> {
    let root = root_task(snapshot).ok_or_else(|| runtime("a settled run has no root execution"))?;
    let message = process_message(snapshot, &root);
    let mut inputs = Vec::new();
    for (name, value) in message.outputs.iter() {
        if name == consts::ACT_USE_PARENT_PROC_ID
            || name == consts::ACT_USE_PARENT_TASK_ID
            || is_error_fact(name)
        {
            continue;
        }
        inputs.push((name.clone(), value.to_string()));
    }
    Ok(inputs)
}

/// The subflow child's return operation.
pub(super) struct ReturnToParent;

#[async_trait::async_trait]
impl ActOperation for ReturnToParent {
    /// The hand-back only has work once the child settled and the parent is
    /// waiting for it. Anything else defers without consuming the operation.
    async fn precheck(&self, host: &KernelHost, pending: &Pending<'_>) -> Result<Precheck> {
        if run_outcome(pending.snapshot).is_none() {
            return Ok(Precheck::Ignore);
        }
        let Some(link) = parent_link(pending.snapshot)? else {
            return Ok(Precheck::Proceed);
        };
        let mut state = parent_state(host, &link).await?;
        if matches!(state, ParentState::Pending) {
            for _ in 0..PARENT_WAIT_ROUNDS {
                tokio::time::sleep(std::time::Duration::from_millis(PARENT_WAIT_MS)).await;
                state = parent_state(host, &link).await?;
                if !matches!(state, ParentState::Pending) {
                    break;
                }
            }
        }
        Ok(match state {
            ParentState::Pending => Precheck::Ignore,
            ParentState::Waiting | ParentState::Done => Precheck::Proceed,
        })
    }

    async fn run(&self, host: &KernelHost, operation: &mut Operation<'_>) -> Result<Outcome> {
        let snapshot = operation.snapshot;
        if run_outcome(snapshot).is_none() {
            return Ok(Outcome::Nothing);
        }
        let Some(link) = parent_link(snapshot)? else {
            // No parent to return to (a malformed seed, or a reclaimed link):
            // the operation retires without a decision.
            return Ok(Outcome::Planned);
        };
        match parent_state(host, &link).await? {
            ParentState::Done => Ok(Outcome::Planned),
            ParentState::Pending => Ok(Outcome::Nothing),
            ParentState::Waiting => {
                let action = return_action(snapshot)?;
                let inputs = return_inputs(snapshot)?;
                match host
                    .apply_action(&link.pid, &link.tid, action, inputs)
                    .await
                {
                    Ok(()) => Ok(Outcome::Planned),
                    Err(error) => {
                        // A concurrent decision on the parent (or its
                        // reclamation) may have made the hand-back moot. Only
                        // then is the refusal a success; otherwise the
                        // operation stays open for recovery.
                        if matches!(parent_state(host, &link).await?, ParentState::Done) {
                            Ok(Outcome::Planned)
                        } else {
                            Err(error)
                        }
                    }
                }
            }
        }
    }

    async fn next(
        &self,
        host: &KernelHost,
        operation: &Operation<'_>,
        _outcome: Outcome,
    ) -> Result<TransitionPlan> {
        // The operation's own commit: no state change, no propagation. The
        // empty payload keeps the note from becoming an engine message.
        host.task_operation_plan(
            operation.snapshot,
            operation.id,
            &operation.task_id,
            operation.task,
            operation.claim,
            "returned",
            "task.completed",
        )
    }
}

impl KernelHost {
    /// Ensure the child aggregate carries its durable hand-back. Called once
    /// the committed snapshot reports the child terminal; the existing
    /// operation (completed or not) is the idempotency marker, so a replayed
    /// terminal report never creates a second one.
    pub(in crate::scheduler::shell) async fn ensure_parent_return(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    ) -> Result<()> {
        // Only a settled child owes a return; a live one keeps running.
        if run_outcome(snapshot).is_none() || parent_link(snapshot)?.is_none() {
            return Ok(());
        }
        let Some(root) = root_task(snapshot) else {
            return Ok(());
        };
        let mut current = snapshot.clone();
        for _ in 0..3 {
            if current.operations.iter().any(|(id, operation)| {
                id.task == root && operation.kind == OperationKind::ReturnToParent
            }) {
                return Ok(());
            }
            let id = OperationId {
                process_id: current.process_id.clone(),
                task: root.clone(),
                sequence: current.next_operation_sequence(root.node.as_str(), root.generation),
            };
            let plan = TransitionPlan {
                process_id: current.process_id.clone(),
                expected: current.version.clone(),
                task_creates: Vec::new(),
                state_changes: Vec::new(),
                variable_writes: Vec::new(),
                source_operation: None,
                operations: vec![OperationIntent {
                    id: id.clone(),
                    kind: OperationKind::ReturnToParent,
                    ready_at_ms: None,
                }],
                outbox: Vec::new(),
            };
            plan.validate().map_err(runtime)?;
            let outcome = match self
                .processes
                .commit_transition(&plan, SystemClock.now_ms(), self.timer_poll_ms)
                .await
            {
                Ok(outcome) => outcome,
                Err(error) => {
                    let error = repository_error(error);
                    if !is_conflict(&error) {
                        return Err(error);
                    }
                    // Re-read and re-check the marker before minting another
                    // sequence.
                    current = match self.load(&current.process_id).await? {
                        Some(loaded) => loaded.snapshot,
                        None => return Ok(()),
                    };
                    continue;
                }
            };
            match outcome {
                crate::scheduler::durable::async_document_store::TransitionCommitOutcome::Committed { .. }
                | crate::scheduler::durable::async_document_store::TransitionCommitOutcome::AlreadyCommitted { .. } => {
                    self.wake(&id);
                    return Ok(());
                }
                crate::scheduler::durable::async_document_store::TransitionCommitOutcome::Conflict { .. } => {
                    // A concurrent commit moved the aggregate: re-read and
                    // re-check the marker before minting another sequence.
                    current = match self.load(&current.process_id).await? {
                        Some(loaded) => loaded.snapshot,
                        None => return Ok(()),
                    };
                }
            }
        }
        Err(runtime(
            "the child's return operation could not be recorded after three attempts",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Vars;
    use crate::scheduler::core::model::task_graph::TaskId;
    use crate::scheduler::shell::ProcessSpec;

    /// One settled child aggregate with the parent linkage and a user output on
    /// its root scope.
    fn settled(state: TaskState) -> ProcessSnapshot {
        let mut vars = Vars::new()
            .with("out", "kept")
            .with(consts::ACT_USE_PARENT_PROC_ID, "parent")
            .with(consts::ACT_USE_PARENT_TASK_ID, "parent-tid")
            .with("ecode", "E1")
            .with("message", "boom")
            .with("error.code", "E1")
            .with("error.message", "boom");
        vars.set("v", 42);
        let mut snapshot = ProcessSpec::linear("p", crate::utils::consts::TASK_ROOT_TID, &[])
            .with_vars(&vars)
            .seed_snapshot()
            .unwrap();
        let root = TaskId::new(crate::utils::consts::TASK_ROOT_TID, 0).unwrap();
        snapshot.tasks.get_mut(&root).unwrap().state = state;
        snapshot
    }

    fn root(snapshot: &ProcessSnapshot) -> TaskId {
        root_task(snapshot).unwrap()
    }

    #[test]
    fn return_to_parent_maps_the_childs_terminal_state_to_the_parents_action() {
        assert_eq!(
            return_action(&settled(TaskState::Completed)).unwrap(),
            ActionKind::Complete
        );
        assert_eq!(
            return_action(&settled(TaskState::Failed)).unwrap(),
            ActionKind::Fail {
                code: "E1".into(),
                message: "boom".into(),
            }
        );
        assert_eq!(
            return_action(&settled(TaskState::Aborted)).unwrap(),
            ActionKind::Abort
        );
        assert_eq!(
            return_action(&settled(TaskState::Skipped)).unwrap(),
            ActionKind::Skip
        );
    }

    #[test]
    fn return_to_parent_folds_the_childs_outputs_and_drops_the_linkage() {
        let snapshot = settled(TaskState::Completed);
        let inputs = return_inputs(&snapshot).unwrap();
        assert!(
            inputs.contains(&("out".to_string(), "\"kept\"".to_string())),
            "the child's output rides the action: {inputs:?}"
        );
        assert!(
            inputs.contains(&("v".to_string(), "42".to_string())),
            "the child's variable rides the action: {inputs:?}"
        );
        for dropped in [
            consts::ACT_USE_PARENT_PROC_ID,
            consts::ACT_USE_PARENT_TASK_ID,
            "ecode",
            "message",
            "error.code",
            "error.message",
            "auto",
        ] {
            assert!(
                inputs.iter().all(|(name, _)| name != dropped),
                "engine fact '{dropped}' must not reach the parent: {inputs:?}"
            );
        }
    }

    #[tokio::test]
    async fn return_to_parent_is_recorded_once_for_a_settled_child() {
        let (host, _wake) = KernelHost::in_memory();
        let snapshot = settled(TaskState::Completed);
        host.processes
            .create(&snapshot, 0, 10)
            .await
            .expect("the child aggregate is created");

        host.ensure_parent_return(&snapshot).await.unwrap();
        host.ensure_parent_return(&snapshot).await.unwrap();

        let loaded = host.load("p").await.unwrap().expect("the child exists");
        let returns: Vec<_> = loaded
            .snapshot
            .operations
            .iter()
            .filter(|(id, operation)| {
                id.task == root(&snapshot) && operation.kind == OperationKind::ReturnToParent
            })
            .collect();
        assert_eq!(returns.len(), 1, "one hand-back per child run");
    }

    #[tokio::test]
    async fn return_to_parent_records_nothing_for_a_live_child() {
        let (host, _wake) = KernelHost::in_memory();
        let snapshot = settled(TaskState::Running);
        host.processes
            .create(&snapshot, 0, 10)
            .await
            .expect("the child aggregate is created");

        host.ensure_parent_return(&snapshot).await.unwrap();

        let loaded = host.load("p").await.unwrap().expect("the child exists");
        assert!(
            loaded
                .snapshot
                .operations
                .values()
                .all(|operation| operation.kind != OperationKind::ReturnToParent),
            "a run that is still in flight owes no hand-back"
        );
    }
}
