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
//! 4. **Dispatch** ([`dispatch`]): the plan's records delivered, its
//!    transition committed, its new operations woken, and the run's
//!    bookkeeping (lifecycle, read model, slot) settled.
//!
//! Everything kind-specific lives in one module per kind under this one —
//! [`task`], [`action`], [`timeout`], [`cascade`], [`propagate`], [`reentry`]
//! — so this driver never matches on an [`OperationKind`].

mod action;
mod cascade;
mod dispatch;
mod prepare;
mod propagate;
mod reentry;
mod task;
mod timeout;

use self::action::ApplyAction;
use self::cascade::{AbortTask, SkipTask};
pub(super) use self::dispatch::Projection;
use self::propagate::PropagateTask;
use self::reentry::ReenterTask;
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
    pub(super) snapshot: crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    pub(super) now_ms: i64,
}

impl Claimed {
    /// The claimed operation as phases 2 and 3 read it: the identity, the
    /// claim, the authoritative snapshot, and the execution the operation
    /// names (absent for an operation whose execution a decision removed).
    pub(super) fn operation(&self) -> Operation<'_> {
        let task_id = self.id.task.clone();
        let task = self.snapshot.tasks.get(&task_id);
        Operation {
            id: &self.id,
            claim: &self.claim,
            task_id,
            task,
            snapshot: &self.snapshot,
            now_ms: self.now_ms,
        }
    }
}

/// Phases 2 and 3's view of one claimed operation: everything a kind's work and its
/// plan need, borrowed from the aggregate the claim handed back.
pub(super) struct Operation<'a> {
    pub(super) id: &'a OperationId,
    pub(super) claim: &'a OperationCommit,
    pub(super) task_id: crate::scheduler::core::model::task_graph::TaskId,
    pub(super) task: Option<&'a crate::scheduler::core::commit::aggregate::TaskSnapshot>,
    pub(super) snapshot: &'a crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    pub(super) now_ms: i64,
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
    async fn run(&self, host: &KernelHost, operation: &Operation<'_>) -> Result<Outcome>;

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
        operation_trace::operation();
        // Phase 1 — prepare: the slot policy, the kind's pre-claim check, the
        // claim. `None` is "nothing to drive", never an error.
        let Some(claimed) = self.prepare(id, Slot::Policy).await? else {
            return Ok(false);
        };
        let driver = driver(claimed.kind).ok_or_else(|| {
            runtime(format!(
                "operation kind {:?} is not driven by the host yet",
                claimed.kind
            ))
        })?;
        let operation = claimed.operation();
        // Phases 2 and 3 — perform and plan: the kind's own work and the
        // transition that work implies. One timer covers both, exactly the
        // span the pre-split `PLAN` stage measured.
        let phase = operation_trace::start();
        let outcome = driver.run(self, &operation).await?;
        let effect_started = outcome.ran_external_effect();
        if matches!(outcome, Outcome::Nothing) {
            operation_trace::end(operation_trace::PLAN, phase);
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
            Ok(plan) => {
                operation_trace::end(operation_trace::PLAN, phase);
                plan
            }
            Err(error) => {
                operation_trace::end(operation_trace::PLAN, phase);
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
        // Phase 4 — dispatch: deliver, commit, wake, settle.
        self.dispatch(&claimed, plan, Projection::Touched, effect_started)
            .await?;
        Ok(true)
    }
}
