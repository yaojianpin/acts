//! The operation pipeline: one operation, from claim to its next commit.
//!
//! An operation ([`OperationId`]) is the aggregate's durable unit of work;
//! taking a claimed one to its next commit is what this module does, and the
//! driver's ready-queue turn, a lost wake recovered by the due pass, and a
//! client action's own operation all take the same four phases. Only the middle
//! two depend on the operation's kind:
//!
//! 1. **Prepare** ([`prepare`]): the parked fast path, the slot policy
//!    ([`KernelHost::place_run`]), the kind's own pre-claim policy
//!    ([`ActOperation::precheck`]) and the claim, as one atomic
//!    `ClaimAndPrepare` event. The snapshot that commit hands back is the
//!    authoritative state the rest of the operation is formed from.
//! 2. **Perform** ([`ActOperation::run`]): the operation's own work —
//!    evaluating a step's guard and resolving its act, running a package's
//!    effect, deciding a timer's round, or nothing at all for a claimed operation whose
//!    job is its plan.
//! 3. **ActPlan** ([`ActOperation::next`]): that outcome folded into the
//!    durable next plan — the state changes, the successor operations and the
//!    outbox records it implies.
//! 4. **Dispatch** ([`dispatch`]): the transition committed (its outbox
//!    records written `Pending`), then Core's complete
//!    [`Decision`](crate::scheduler::core::decision::Decision) for the
//!    committed snapshot executed in order — project its rows, report its
//!    lifecycle, deliver and confirm the plan's outbox records in one batch,
//!    wake its new operations and apply its admission directive. The shell
//!    never re-derives settlement, lifecycle or admission; those rules are
//!    `core::decision`'s.
//!
//! Everything kind-specific lives in one module per kind under this one —
//! [`task`], [`action`], [`timeout`], [`cascade`], [`propagate`], [`reentry`],
//! [`return_to_parent`] — so this driver never matches on an [`OperationKind`].

mod action;
mod cascade;
mod dispatch;
mod fold;
mod join;
mod prepare;
mod propagate;
mod reentry;
mod return_to_parent;
mod task;
mod timeout;

use self::action::ApplyAction;
use self::cascade::{AbortTask, SkipTask};
pub(super) use self::dispatch::Projection;
use self::propagate::PropagateTask;
use self::reentry::ReenterTask;
use self::return_to_parent::ReturnToParent;
use self::task::ExecuteTask;
use self::timeout::DispatchTimeout;

use super::*;

/// Phase 1's verdict on whether this operation has anything to do at all.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Precheck {
    /// Drive the operation: claim the operation and run its work.
    Proceed,
    /// Nothing to do *before the claim*: leave the operation exactly as it is —
    /// its state, its claim — and let the next tick re-derive it.
    Ignore,
}

/// Whether phase 1 applies the run's slot policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Slot {
    /// The driver's own operations: the parked fast path and the slot policy
    /// ([`KernelHost::place_run`]) decide whether this run may operation at all.
    Policy,
    /// The client action path: the decision is already durable — the intent was
    /// committed before the operation — so the operation is claimed directly. The
    /// parked queue books the runs the *driver* drives; it is not a gate on a
    /// decision a client has already been told about.
    Direct,
}

/// A4's single-writer turn over one pid: the command-lane guard a command
/// holds across its claim, its plan and its commit — released for exactly one
/// window, the external effect itself.
///
/// The lane is what keeps two plans of one process from racing on the same
/// revision: phases 1, 3 and 4 run inside the turn, so the next command of the
/// same pid always loads the state this one committed. Phase 2 hands the lane
/// back before `EffectRunner::run` and retakes it right after, so an effect
/// never serializes the process's other commands — and a command that advanced
/// the aggregate while the effect was in flight makes the effect's commit lose
/// its guard, which is exactly the `EffectUnknown` boundary A1a/A1c own. What
/// the lane cannot see — a crash between claim and commit, a lease expiry,
/// another instance — stays fenced by the aggregate's revision guards.
pub(super) struct CommandLane {
    host: KernelHost,
    process_id: String,
    guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl CommandLane {
    /// Acquire the pid's command lane and hold it.
    pub(super) async fn hold(host: &KernelHost, process_id: &str) -> Self {
        let guard = host.lock_command_lane(process_id).await;
        Self {
            host: host.clone(),
            process_id: process_id.to_string(),
            guard: Some(guard),
        }
    }

    /// A lane that belongs to no command: the fold plans its scratch
    /// operations inside the producing command's own turn (see
    /// `fold_internal`) and they never run an effect, so they hold no lane of
    /// their own. `release`/`resume` on it would be a bug — only `run` uses
    /// those, and the fold never calls `run`.
    pub(super) fn detached(host: &KernelHost) -> Self {
        Self {
            host: host.clone(),
            process_id: String::new(),
            guard: None,
        }
    }

    /// Hand the lane back for the external effect. Nothing may be awaited
    /// between this and [`Self::resume`] — the window is the effect itself.
    pub(super) fn release(&mut self) {
        self.guard = None;
    }

    /// Retake the lane the effect released; commands of the same pid that
    /// decided in the meantime are what a lost plan-commit guard now means.
    pub(super) async fn resume(&mut self) {
        if self.guard.is_none() {
            self.guard = Some(self.host.lock_command_lane(&self.process_id).await);
        }
    }
}

/// Phase 1's pre-claim view of one operation: the identity, the state it was
/// read from and the clock the operation runs on. A kind's pre-claim policy may look
/// at the aggregate, but it must not write anything — the claim has not been
/// taken yet.
pub(super) struct Pending<'a> {
    pub(super) id: &'a OperationId,
    pub(super) snapshot: &'a crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    pub(super) now_ms: i64,
}

/// Phase 1's product: one operation claimed and prepared, with the
/// authoritative snapshot its plan must be formed from.
pub(super) struct Claimed {
    pub(super) id: OperationId,
    pub(super) kind: OperationKind,
    /// The claim, as the plan's `source_operation`: committing the plan closes
    /// the operation inside the same atomic batch.
    pub(super) claim: OperationCommit,
    /// The prepared record's revision: what a claimed operation that cannot commit its plan
    /// releases its claim against ([`prepare::release_claim`]).
    pub(super) revision: u64,
    /// The aggregate the claim committed, with the exact document bytes it
    /// was read as. Phase 4 commits the plan against this same copy instead of
    /// decoding the whole aggregate a second time: the durable documents are
    /// its CAS token, and the claim is the only writer that could have moved
    /// them (the command lane holds the pid). `None` once dispatch took it.
    pub(super) loaded: Option<crate::scheduler::durable::document_store::LoadedProcess>,
    pub(super) now_ms: i64,
}

impl Claimed {
    pub(super) fn snapshot(&self) -> &crate::scheduler::core::commit::aggregate::ProcessSnapshot {
        self.loaded
            .as_ref()
            .expect("the claimed aggregate is present until dispatch takes it")
            .snapshot
            .as_ref()
    }

    /// The claimed aggregate's shared handle. The operation clones the [`Arc`],
    /// so a reader can hold the exact snapshot it planned against without
    /// copying its tasks or variables.
    pub(super) fn snapshot_arc(
        &self,
    ) -> std::sync::Arc<crate::scheduler::core::commit::aggregate::ProcessSnapshot> {
        std::sync::Arc::clone(
            &self
                .loaded
                .as_ref()
                .expect("the claimed aggregate is present until dispatch takes it")
                .snapshot,
        )
    }

    /// Hand the claimed aggregate to phase 4, which commits the plan against
    /// it. The rest of the claim (identity, token, revision) stays readable.
    pub(super) fn take_loaded(
        &mut self,
    ) -> crate::scheduler::durable::document_store::LoadedProcess {
        self.loaded
            .take()
            .expect("the claimed aggregate is taken once, by dispatch")
    }

    /// The claimed operation as phases 2 and 3 read it: the identity, the
    /// claim, the authoritative snapshot, and the execution the operation
    /// names (absent for an operation whose execution a decision removed) —
    /// plus the command lane this operation's effect phase may temporarily
    /// release (A4, [`CommandLane`]).
    pub(super) fn operation<'a>(&'a self, lane: &'a mut CommandLane) -> Operation<'a> {
        let snapshot = self.snapshot_arc();
        let task = snapshot.tasks.get(&self.id.task).cloned();
        Operation {
            id: &self.id,
            claim: &self.claim,
            task_id: self.id.task.clone(),
            task,
            snapshot,
            now_ms: self.now_ms,
            lane,
        }
    }
}

/// Phases 2 and 3's view of one claimed operation: everything a kind's work and its
/// plan need, borrowed from the aggregate the claim handed back.
pub(super) struct Operation<'a> {
    pub(super) id: &'a OperationId,
    pub(super) claim: &'a OperationCommit,
    pub(super) task_id: crate::scheduler::core::model::task_graph::TaskId,
    /// The execution's own facts at claim time, cloned (it is small) so the
    /// operation does not borrow from the shared snapshot. `None` when the
    /// execution no longer exists.
    pub(super) task: Option<crate::scheduler::core::commit::aggregate::TaskSnapshot>,
    /// The aggregate the operation planned against. Shared rather than
    /// borrowed, so a reader can keep the same snapshot after the operation's
    /// claim is handed to dispatch, without a copy.
    pub(super) snapshot: std::sync::Arc<crate::scheduler::core::commit::aggregate::ProcessSnapshot>,
    pub(super) now_ms: i64,
    /// The command lane this operation's pid is decided under (A4). A kind
    /// whose phase 2 crosses the external-effect boundary releases it for the
    /// effect and resumes after; everything else leaves it held.
    pub(super) lane: &'a mut CommandLane,
}

impl Operation<'_> {
    /// The authoritative snapshot this operation plans against.
    pub(super) fn snapshot(&self) -> &crate::scheduler::core::commit::aggregate::ProcessSnapshot {
        self.snapshot.as_ref()
    }

    /// The execution this operation names, absent for an operation whose
    /// execution a decision removed.
    pub(super) fn task(&self) -> Option<&crate::scheduler::core::commit::aggregate::TaskSnapshot> {
        self.task.as_ref()
    }
}

/// Phase 2's product: what running one operation decided. Each variant is the
/// kind's own vocabulary — the driver below only carries it from
/// [`ActOperation::run`] to [`ActOperation::next`].
pub(super) enum Outcome {
    /// `ExecuteTask`: the act's start was decided.
    Start(task::Start),
    /// `ApplyAction`: a durable decision replayed and/or the act's effect ran.
    Effect(action::Effect),
    /// `DispatchTimeout`: the timer's round (never an idle one — that is
    /// [`Outcome::Nothing`]).
    Timeout(TimeoutRound),
    /// The kind's whole work is its plan (`SkipTask`, `AbortTask`,
    /// `PropagateTask`, `ReenterTask`): nothing ran, the snapshot is the input.
    Planned,
    /// Nothing to commit after all (a timer round that turned idle between the
    /// preflight and the claim): the operation's claim goes back and the operation
    /// waits for the next tick.
    Nothing,
}

impl Outcome {
    /// Whether phase 2 crossed the external-effect boundary.
    ///
    /// A package that returned success *or failure* may already have touched
    /// the outside world; an internal plan, a client decision and an idle
    /// timer round did not. The distinction is what lets dispatch decide
    /// whether a lost compare-and-swap may retry the operation or must surface
    /// it as an unknown effect.
    pub(super) fn ran_external_effect(&self) -> bool {
        matches!(
            self,
            Outcome::Effect(action::Effect::Completed(Some(_)) | action::Effect::Failed(_))
        )
    }
}

/// One operation kind's execution: the kind-specific half of driving one.
///
/// The host runs the four phases above; only the pre-claim policy and the
/// middle two phases depend on the kind, and each kind answers them in its own
/// module under this one. The host is passed in rather than held: the drivers
/// are stateless, and every fact they work with is in the [`Operation`].
#[async_trait::async_trait]
pub(super) trait ActOperation: Send + Sync {
    /// Phase 1's kind-specific half, decided before the claim consumes
    /// anything.
    async fn precheck(&self, host: &KernelHost, pending: &Pending<'_>) -> Result<Precheck>;

    /// Phase 2: the operation's own work, reported as the outcome phase 3
    /// folds into the operation's plan.
    async fn run(&self, host: &KernelHost, operation: &mut Operation<'_>) -> Result<Outcome>;

    /// Phase 3: the durable next plan of one outcome. [`Outcome::Nothing`]
    /// never reaches here — that operation has no plan at all.
    async fn next(
        &self,
        host: &KernelHost,
        operation: &Operation<'_>,
        outcome: Outcome,
    ) -> Result<TransitionPlan>;
}

/// The driver of one operation kind, or `None` for a kind this host does not
/// drive yet (the kinds the engine only persists as vocabulary:
/// `DeliverMessage`, `FireTrigger`, `RunPostCommit`).
pub(super) fn driver(kind: OperationKind) -> Option<&'static dyn ActOperation> {
    match kind {
        OperationKind::ExecuteTask => Some(&ExecuteTask),
        OperationKind::ApplyAction => Some(&ApplyAction),
        OperationKind::SkipTask => Some(&SkipTask),
        OperationKind::AbortTask => Some(&AbortTask),
        OperationKind::DispatchTimeout => Some(&DispatchTimeout),
        OperationKind::PropagateTask => Some(&PropagateTask),
        OperationKind::ReenterTask => Some(&ReenterTask),
        OperationKind::ReturnToParent => Some(&ReturnToParent),
        OperationKind::DeliverMessage
        | OperationKind::FireTrigger
        | OperationKind::RunPostCommit => None,
    }
}

impl KernelHost {
    /// One durable operation, in the four phases above. Returns `Ok(false)` when the
    /// operation needed nothing — a wake redelivered for an already-terminal
    /// operation, a run parked by the slot cap, or a timer with nothing due.
    pub(in crate::scheduler::shell) async fn execute_one(&self, id: &OperationId) -> Result<bool> {
        // A4: this turn holds the pid's command lane — claim, plan and commit
        // decide inside it, so no client action or recovery disposition of
        // the same run plans beside this operation on the same revision. The
        // lane is released only around the external effect (see
        // [`CommandLane`]); every early return below drops it.
        let mut lane = CommandLane::hold(self, &id.process_id).await;
        // Phase 1 — prepare: the slot policy, the kind's pre-claim check, the
        // claim. `None` is "nothing to drive", never an error.
        let Some(mut claimed) = self.prepare(id, Slot::Policy).await? else {
            return Ok(false);
        };
        let driver = driver(claimed.kind).ok_or_else(|| {
            runtime(format!(
                "operation kind {:?} is not driven by the host yet",
                claimed.kind
            ))
        })?;
        let mut operation = claimed.operation(&mut lane);
        // Phases 2 and 3 — perform and plan: the kind's own work and the
        // transition that work implies.
        let outcome = driver.run(self, &mut operation).await?;
        let effect_started = outcome.ran_external_effect();
        if matches!(outcome, Outcome::Nothing) {
            // The work turned out to be nothing after all: the claim goes back
            // and the release writes the ready record the driver defers to the
            // next poll. The release is best-effort — recovery's lease expiry
            // leads to the same record and the same "nothing happened" answer.
            if let Err(error) = self.release_claim(&claimed, 1).await {
                tracing::debug!(
                    id = ?claimed.id,
                    error = %error,
                    "releasing an idle operation's claim failed; recovery's lease expiry decides it"
                );
            }
            return Ok(false);
        }
        let plan = match driver.next(self, &operation, outcome).await {
            Ok(plan) => plan,
            Err(error) => {
                if effect_started && let Err(mark_error) = self.mark_effect_unknown(&claimed).await
                {
                    tracing::warn!(
                        id = ?claimed.id,
                        error = %mark_error,
                        "marking an effect-unknown operation failed; its lease expiry will decide it"
                    );
                }
                return Err(error);
            }
        };
        // Phase 4 — dispatch: commit, project, deliver, wake, settle.
        self.dispatch(&mut claimed, plan, Projection::Touched, effect_started)
            .await?;
        Ok(true)
    }

    /// Converge an `EffectUnknown` operation from an executor observation
    /// that proves the effect ran (A1c): re-plan the operation from the
    /// observed outcome and commit it **without** calling the effect again.
    ///
    /// Returns `Ok(false)` when the observation is not an applied proof or the
    /// operation is no longer claimable. The dispatch is marked as "effect
    /// started" even though this attempt did not run it: a lost commit must
    /// return the operation to `EffectUnknown`, never release it for an
    /// automatic re-drive that would run the effect a second time.
    pub(in crate::scheduler::shell) async fn converge_observed_effect(
        &self,
        id: &OperationId,
        observation: effects::EffectObservation,
    ) -> Result<bool> {
        let effect = match observation {
            effects::EffectObservation::Applied(outcome) => {
                action::Effect::Completed(Some(outcome))
            }
            effects::EffectObservation::Failed(message) => {
                action::Effect::Failed(ActError::Package(message))
            }
            effects::EffectObservation::NotApplied | effects::EffectObservation::Unknown => {
                return Ok(false);
            }
        };
        let mut lane = CommandLane::hold(self, &id.process_id).await;
        let Some(mut claimed) = self.prepare_reconciliation(id).await? else {
            return Ok(false);
        };
        let operation = claimed.operation(&mut lane);
        let plan = match ApplyAction
            .next(self, &operation, Outcome::Effect(effect))
            .await
        {
            Ok(plan) => plan,
            Err(error) => {
                if let Err(mark_error) = self.mark_effect_unknown(&claimed).await {
                    tracing::warn!(
                        id = ?claimed.id,
                        error = %mark_error,
                        "marking an observed effect's failed replan as unknown failed; its lease expiry will decide it"
                    );
                }
                return Err(error);
            }
        };
        self.dispatch(&mut claimed, plan, Projection::Touched, true)
            .await?;
        Ok(true)
    }

    /// Return an `EffectUnknown` operation to the retry queue under its own
    /// attempt. Only called for a provably idempotent effect whose executor
    /// observation says it did not run, or for an operator's explicit
    /// "not applied" resolution. The operation identity — and therefore its
    /// effect key — does not change.
    pub(in crate::scheduler::shell) async fn retry_unknown_effect(
        &self,
        operation: &crate::scheduler::core::model::operation::OperationRecord,
    ) -> Result<bool> {
        let OperationState::EffectUnknown { attempt } = &operation.state else {
            return Ok(false);
        };
        let attempt = *attempt;
        // A4: the re-queue is a command of the same pid — it takes the lane,
        // while the caller's revision guard stays the fence against a state
        // that moved since the observation was taken.
        let _lane = self.lock_command_lane(&operation.id.process_id).await;
        let (evolved, _) = self
            .operation_event(
                &operation.id,
                operation.revision,
                OperationEvent::ResolveEffectNotApplied { attempt },
            )
            .await?;
        self.wake(&evolved.id);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::core::model::{
        operation::{OperationRecord, OperationState},
        task_graph::{TaskId, TaskState},
    };

    fn host() -> std::sync::Arc<KernelHost> {
        KernelHost::in_memory().0
    }

    fn op(node: &str, sequence: u64, kind: OperationKind) -> OperationId {
        let id = OperationId {
            process_id: "p".into(),
            task: TaskId::new(node, 0).unwrap(),
            sequence,
        };
        let _ = kind;
        id
    }

    async fn persist(host: &KernelHost, snapshot: &ProcessSnapshot) {
        host.processes
            .create(snapshot, SystemClock.now_ms(), host.timer_poll_ms())
            .await
            .expect("the seed aggregate is created");
    }

    fn seeded() -> ProcessSnapshot {
        ProcessSpec::linear("p", "root", &["a"])
            .seed_snapshot()
            .unwrap()
    }

    /// Every driven kind has its own driver; the persisted vocabulary kinds
    /// have none yet.
    #[test]
    fn the_driver_table_names_every_kind() {
        for kind in [
            OperationKind::ExecuteTask,
            OperationKind::ApplyAction,
            OperationKind::SkipTask,
            OperationKind::AbortTask,
            OperationKind::DispatchTimeout,
            OperationKind::PropagateTask,
            OperationKind::ReenterTask,
            OperationKind::ReturnToParent,
        ] {
            assert!(driver(kind).is_some(), "{kind:?} must be driven");
        }
        for kind in [
            OperationKind::DeliverMessage,
            OperationKind::FireTrigger,
            OperationKind::RunPostCommit,
        ] {
            assert!(driver(kind).is_none(), "{kind:?} is vocabulary only");
        }
    }

    /// A command lane handed back for the effect can be retaken.
    #[tokio::test]
    async fn a_released_lane_is_resumed() {
        let host = host();
        let mut lane = CommandLane::hold(&host, "p").await;
        lane.release();
        lane.resume().await;
        // The detached lane never held anything and must not try to resume.
        let mut detached = CommandLane::detached(&host);
        assert_eq!(detached.process_id, "");
        detached.release();
    }

    /// A kernel that does not drive a persisted kind fails the operation
    /// loudly instead of silently dropping it.
    #[tokio::test]
    async fn an_undriven_kind_is_an_error() {
        let host = host();
        let mut snapshot = seeded();
        let id = op("a", 0, OperationKind::DeliverMessage);
        snapshot.operations.insert(
            id.clone(),
            OperationRecord::pending(id.clone(), OperationKind::DeliverMessage),
        );
        persist(&host, &snapshot).await;
        let error = host.execute_one(&id).await.unwrap_err();
        assert!(format!("{error}").contains("not driven"), "{error}");
    }

    /// The start's own plan fails when the execution it names is gone; the
    /// failure surfaces and the operation stays open for recovery.
    #[tokio::test]
    async fn a_start_whose_execution_is_gone_fails() {
        let host = host();
        let mut snapshot = seeded();
        let id = op("ghost", 0, OperationKind::ExecuteTask);
        snapshot.operations.insert(
            id.clone(),
            OperationRecord::pending(id.clone(), OperationKind::ExecuteTask),
        );
        persist(&host, &snapshot).await;
        let error = host.execute_one(&id).await.unwrap_err();
        assert!(format!("{error}").contains("not found"), "{error}");
    }

    /// An observation that proves nothing leaves the operation alone.
    #[tokio::test]
    async fn unproven_observations_are_ignored() {
        let host = host();
        for observation in [
            crate::scheduler::shell::effects::EffectObservation::NotApplied,
            crate::scheduler::shell::effects::EffectObservation::Unknown,
        ] {
            assert!(
                !host
                    .converge_observed_effect(&op("a", 0, OperationKind::ApplyAction), observation)
                    .await
                    .unwrap()
            );
        }
    }

    /// An unknown effect whose aggregate is gone has nothing to converge.
    #[tokio::test]
    async fn a_missing_aggregate_cannot_converge() {
        let host = host();
        assert!(
            !host
                .converge_observed_effect(
                    &op("a", 0, OperationKind::ApplyAction),
                    crate::scheduler::shell::effects::EffectObservation::Failed("boom".into()),
                )
                .await
                .unwrap()
        );
    }

    fn unknown_apply_action(snapshot: &mut ProcessSnapshot) -> OperationId {
        let id = op("a", 0, OperationKind::ApplyAction);
        let mut record = OperationRecord::pending(id.clone(), OperationKind::ApplyAction);
        record.state = OperationState::EffectUnknown { attempt: 1 };
        snapshot.operations.insert(id.clone(), record);
        snapshot
            .tasks
            .get_mut(&TaskId::new("a", 0).unwrap())
            .unwrap()
            .state = TaskState::Running;
        id
    }

    /// An executor's failure proof is replayed through the failure plan.
    #[tokio::test]
    async fn a_failed_observation_fails_the_execution() {
        let host = host();
        let mut snapshot = seeded();
        let id = unknown_apply_action(&mut snapshot);
        persist(&host, &snapshot).await;
        assert!(
            host.converge_observed_effect(
                &id,
                crate::scheduler::shell::effects::EffectObservation::Failed("boom".into()),
            )
            .await
            .unwrap()
        );
        let snapshot = host.snapshot("p").await.unwrap().unwrap();
        assert_eq!(
            snapshot.tasks[&TaskId::new("a", 0).unwrap()].state,
            TaskState::Failed
        );
    }

    /// An applied observation completes the execution without running the
    /// effect again.
    #[tokio::test]
    async fn an_applied_observation_completes_the_execution() {
        let host = host();
        let mut snapshot = seeded();
        let id = unknown_apply_action(&mut snapshot);
        persist(&host, &snapshot).await;
        assert!(
            host.converge_observed_effect(
                &id,
                crate::scheduler::shell::effects::EffectObservation::Applied(
                    crate::scheduler::shell::effects::EffectOutcome {
                        outputs: None,
                        decision: None,
                    },
                ),
            )
            .await
            .unwrap()
        );
        let snapshot = host.snapshot("p").await.unwrap().unwrap();
        assert_eq!(
            snapshot.tasks[&TaskId::new("a", 0).unwrap()].state,
            TaskState::Completed
        );
    }

    /// Only an `EffectUnknown` operation may be re-queued as not applied.
    #[tokio::test]
    async fn a_known_operation_is_not_requeued() {
        let host = host();
        let mut snapshot = seeded();
        let id = op("a", 0, OperationKind::ApplyAction);
        snapshot.operations.insert(
            id.clone(),
            OperationRecord::pending(id.clone(), OperationKind::ApplyAction),
        );
        persist(&host, &snapshot).await;
        let record = host.snapshot("p").await.unwrap().unwrap().operations[&id].clone();
        assert!(!host.retry_unknown_effect(&record).await.unwrap());
    }

    /// A proven-not-applied unknown effect returns to the retry queue.
    #[tokio::test]
    async fn a_not_applied_unknown_effect_is_requeued() {
        let host = host();
        let mut snapshot = seeded();
        let id = unknown_apply_action(&mut snapshot);
        persist(&host, &snapshot).await;
        let record = host.snapshot("p").await.unwrap().unwrap().operations[&id].clone();
        assert!(host.retry_unknown_effect(&record).await.unwrap());
        let snapshot = host.snapshot("p").await.unwrap().unwrap();
        assert!(matches!(
            snapshot.operations[&id].state,
            OperationState::RetryScheduled { attempt: 1 }
        ));
    }

    /// An effect-free decision whose plan fails surfaces the error directly:
    /// nothing crossed the effect boundary, so the operation is not marked
    /// unknown.
    #[tokio::test]
    async fn an_effect_free_plan_error_surfaces_without_marking_unknown() {
        let host = host();
        let mut snapshot = seeded();
        let task_id = TaskId::new("a", 0).unwrap();
        snapshot.tasks.get_mut(&task_id).unwrap().state = TaskState::Waiting;
        snapshot.variables.insert(
            (task_id.clone(), "__action@0".to_string()),
            serde_json::json!({ "kind": { "action": "back" }, "inputs": [] }).to_string(),
        );
        let id = op("a", 0, OperationKind::ApplyAction);
        snapshot.operations.insert(
            id.clone(),
            OperationRecord::pending(id.clone(), OperationKind::ApplyAction),
        );
        persist(&host, &snapshot).await;
        let error = host.execute_one(&id).await.unwrap_err();
        assert!(format!("{error}").contains("'to'"), "{error}");
        let snapshot = host.snapshot("p").await.unwrap().unwrap();
        assert!(
            matches!(
                snapshot.operations[&id].state,
                OperationState::CommitPrepared(_)
            ),
            "an effect-free failure stays open for recovery: {:?}",
            snapshot.operations[&id].state
        );
    }

    /// An observed failure whose replan fails marks the operation unknown: the
    /// effect ran, so recovery must reconcile rather than replay it.
    #[tokio::test]
    async fn a_failed_observation_whose_replan_fails_is_marked_unknown() {
        let host = host();
        let workflow = crate::model::Workflow::new()
            .with_step(|step| step.with_id("s1").with_catch(|catch| catch.with_id("c1")));
        let mut snapshot = ProcessSpec::from_workflow("p", &workflow, &|_| None)
            .unwrap()
            .seed_snapshot()
            .unwrap();
        let task_id = TaskId::new("s1", 0).unwrap();
        snapshot.tasks.get_mut(&task_id).unwrap().state = TaskState::Running;
        let id = op("s1", 0, OperationKind::ApplyAction);
        let mut record = OperationRecord::pending(id.clone(), OperationKind::ApplyAction);
        record.state = OperationState::EffectUnknown { attempt: 1 };
        snapshot.operations.insert(id.clone(), record);
        // Break the auto map so the catch fan-out cannot plan.
        let root = snapshot
            .tasks
            .iter()
            .find(|(_, task)| task.parent.is_none())
            .map(|(id, _)| id.clone())
            .unwrap();
        let key = (
            root,
            crate::scheduler::shell::model::AUTO_MAP_VAR.to_string(),
        );
        let raw = snapshot.variables.get(&key).unwrap();
        let mut map: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(raw).unwrap();
        map.remove("c1");
        snapshot
            .variables
            .insert(key, serde_json::Value::Object(map).to_string());
        persist(&host, &snapshot).await;
        let error = host
            .converge_observed_effect(
                &id,
                crate::scheduler::shell::effects::EffectObservation::Failed("boom".into()),
            )
            .await
            .unwrap_err();
        assert!(format!("{error}").contains("auto map"), "{error}");
        let snapshot = host.snapshot("p").await.unwrap().unwrap();
        assert!(
            matches!(
                snapshot.operations[&id].state,
                OperationState::EffectUnknown { attempt: 2 }
            ),
            "the observed effect is unknown, not retryable: {:?}",
            snapshot.operations[&id].state
        );
    }
}
