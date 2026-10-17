//! Phase 4 of a claimed operation: dispatching the plan phase 3 built.
//!
//! One commit is the operation's whole durable effect: the plan's outbox records are
//! delivered first (the at-least-once window the effect seam documents), the
//! transition is committed with the operation's own claim as its source operation —
//! which closes the operation *inside* the same atomic batch and writes the
//! ready records of its successors — and only then do the operations the commit
//! created get woken, the run's lifecycle reported, the read model re-materialized
//! and the slot bookkeeping settled.

use super::*;
use crate::scheduler::shell::*;

/// Which rows the read model re-materializes from the committed snapshot.
pub(in crate::scheduler::shell) enum Projection {
    /// Only the executions the operation's own plan named (plus the proc row): the
    /// rows of a run's other executions cannot have changed.
    Touched,
    /// The run's rows whole. The client action path takes this one: an action
    /// is decided against the aggregate as a whole rather than as one operation's
    /// effect, so its commit is the point where every row of the run may need
    /// re-reading.
    Whole,
}

impl KernelHost {
    /// Record that this operation crossed the external-effect boundary and its
    /// outcome can no longer be proven. Recovery will surface it for manual
    /// reconciliation and must never re-drive it automatically.
    pub(in crate::scheduler::shell) async fn mark_effect_unknown(
        &self,
        claimed: &Claimed,
    ) -> Result<()> {
        let attempt = u32::try_from(claimed.claim.claim.fence)
            .map_err(|_| runtime("operation claim fence exceeds the effect-unknown range"))?;
        self.operation_event(
            &claimed.id,
            claimed.revision,
            OperationEvent::MarkEffectUnknown {
                claim: claimed.claim.claim.clone(),
                attempt,
            },
        )
        .await?;
        Ok(())
    }

    /// Phase 4: deliver, commit, wake and settle. A lost compare-and-swap
    /// releases an effect-free operation's claim for recovery; if phase 2
    /// crossed the external-effect boundary, the operation instead becomes
    /// `EffectUnknown` and is never retried automatically. Every other
    /// failure leaves the operation open for recovery.
    pub(in crate::scheduler::shell) async fn dispatch(
        &self,
        claimed: &Claimed,
        plan: TransitionPlan,
        projection: Projection,
        effect_started: bool,
    ) -> Result<()> {
        let id = &claimed.id;
        let mut plan = plan;
        let phase = operation_trace::start();
        self.deliver_plan(&id.process_id, &mut plan);
        operation_trace::end(operation_trace::DELIVER, phase);
        let phase = operation_trace::start();
        let (receipt, loaded) = match self
            .processes
            .commit_transition(&plan, SystemClock.now_ms(), self.timer_poll_ms)
            .await
            .map_err(repository_error)?
        {
            crate::scheduler::durable::async_document_store::TransitionCommitOutcome::Committed { receipt, snapshot }
            | crate::scheduler::durable::async_document_store::TransitionCommitOutcome::AlreadyCommitted { receipt, snapshot } => (receipt, snapshot),
            crate::scheduler::durable::async_document_store::TransitionCommitOutcome::Conflict { .. } => {
                // A conflict applies nothing, so the *plan* definitely did not
                // commit. An external effect is different: phase 2 may already
                // have run it. Retrying such an operation would repeat the
                // side effect, so it goes to `EffectUnknown` for explicit
                // reconciliation instead of back to the retry queue.
                if effect_started {
                    if let Err(error) = self.mark_effect_unknown(claimed).await {
                        tracing::warn!(
                            id = ?id,
                            error = %error,
                            "marking an effect-unknown operation failed; its lease expiry will decide it"
                        );
                    }
                    return Err(conflict(format!(
                        "commit conflict after an external effect on operation {id:?}; manual reconciliation required"
                    )));
                }
                // Other operations are effect-free: another writer advanced
                // the aggregate and recovery can re-derive this operation
                // against the new state. Releasing the claim here keeps a
                // merely-lost compare-and-swap from waiting out the lease.
                match self.release_claim(claimed, claimed.claim.claim.fence as u32).await {
                    Ok(()) => {
                        // Re-drive against the winner's state now; waiting for
                        // the recovery tick would turn a CAS race into a tick
                        // of latency (and in tests, into a stalled drain).
                        self.wake(&claimed.id);
                    }
                    Err(error) => {
                        tracing::warn!(
                            id = ?id,
                            error = %error,
                            "releasing a conflicted operation's claim failed; recovery's lease expiry decides it"
                        );
                    }
                }
                // The conflict is the retryable race it is: a caller that
                // planned against a snapshot now stale (the client action
                // loop, above all) must be able to tell it from a refusal.
                return Err(conflict(format!("commit conflict on operation {id:?}")));
            }
        };
        operation_trace::end(operation_trace::COMMIT, phase);
        // The commit closed the operation inside its batch (see
        // `advance_transition`): there is no second, close-only commit to run.
        let phase = operation_trace::start();
        for operation in &receipt.new_operations {
            // A timer armed by an execution's *start* operation waits for its
            // first tick: the legacy engine had no round before its first
            // process tick, and the timer's ready record names that poll.
            // A timer a *round* re-armed (the branch chain of a timed-out
            // execution) is woken at once: that is the legacy `Step::next`
            // handing the turn to the branch declared after the one that just
            // ran.
            if claimed.kind == OperationKind::ExecuteTask
                && loaded
                    .operations
                    .get(operation)
                    .is_some_and(|created| created.kind == OperationKind::DispatchTimeout)
            {
                continue;
            }
            self.wake(operation);
        }
        operation_trace::end(operation_trace::WAKE, phase);
        // The run's lifecycle, from the freshly committed state: a start while
        // it is alive, one terminal event once every execution settled. A
        // closed sink or an absent one makes this a no-op.
        let phase = operation_trace::start();
        self.report_lifecycle(&loaded);
        operation_trace::end(operation_trace::LIFECYCLE, phase);
        // The read model follows the same commit — the snapshot that commit
        // returned, never a fresh load: a load is a second read that can
        // already be older than a concurrent operation's commit, and projecting it
        // would write that operation's rows away with an older state (the
        // projection's write order must be the commit order).
        let phase = operation_trace::start();
        match projection {
            Projection::Whole => self.project(&loaded).await,
            Projection::Touched => {
                let touched: std::collections::BTreeSet<String> = plan
                    .state_changes
                    .iter()
                    .map(|change| change.task.wire(&loaded.process_id))
                    .chain(
                        plan.task_creates
                            .iter()
                            .map(|create| create.task.wire(&loaded.process_id)),
                    )
                    .chain(
                        plan.variable_writes
                            .iter()
                            .map(|write| write.scope.wire(&loaded.process_id)),
                    )
                    .collect();
                self.project_touched(&loaded, &touched).await;
            }
        }
        operation_trace::end(operation_trace::PROJECT, phase);
        // This operation may have settled the run: its slot goes back to the parked
        // queue right here, so the run that has waited longest starts now
        // instead of on the driver's next tick. The run is the one this operation
        // just drove, so it either holds its slot or needs none — never parked.
        let phase = operation_trace::start();
        let (_, admission) = self.place_run(&loaded);
        if let Some(admission) = admission {
            self.admit_run(&admission).await?;
        }
        operation_trace::end(operation_trace::PLACE2, phase);
        Ok(())
    }
}
