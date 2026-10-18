//! Phase 4 of a claimed operation: dispatching the plan phase 3 built.
//!
//! One commit is the operation's whole durable effect: the transition is
//! committed with the operation's own claim as its source operation — which
//! closes the operation *inside* the same atomic batch and writes the ready
//! records of its successors — and only then do the operations the commit
//! created get woken, the read model re-materialized, the run's lifecycle
//! reported and the plan's outbox records delivered (the at-least-once window
//! the effect seam documents). Settlement, lifecycle and admission are not
//! decided here: they are the [`Decision`] Core builds from the committed
//! snapshot, and this module only executes its directives in the order the
//! durability contracts require (A5: rows before external observers).

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

    /// Phase 4: commit, project, deliver and settle. The commit writes the
    /// plan's outbox records `Pending`; the projection runs from the committed
    /// snapshot; only then does the transport see the records, and their
    /// outcomes advance in one batch confirmation. A lost compare-and-swap
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
        // Fold every pure-internal hand-off this operation's plan created
        // (`PropagateTask`/`SkipTask`/`AbortTask`/`ReenterTask`) into this same
        // transition. A plan the fold cannot merge is committed as it is and
        // leaves those operations durable for the ready queue, exactly as they
        // were driven before the fold — an effect that already ran must never
        // be made retryable because an internal cascade could not be planned.
        let plan = if plan
            .operations
            .iter()
            .any(|intent| super::fold::folds_into_plan(intent.kind))
        {
            match self
                .fold_internal(&claimed.snapshot, plan.clone(), claimed.now_ms)
                .await
            {
                Ok(folded) => folded,
                Err(error) => {
                    tracing::debug!(
                        id = ?id,
                        error = %error,
                        "folding the plan's internal operations failed; leaving them durable"
                    );
                    plan
                }
            }
        } else {
            plan
        };
        let phase = operation_trace::start();
        let (receipt, mut loaded) = match self
            .processes
            .commit_transition(&plan, SystemClock.now_ms(), self.timer_poll_ms)
            .await
            .map_err(repository_error)
        {
            Err(error) => {
                // The domain refused the plan against the fresh aggregate —
                // a writer committed between this operation's claim and its
                // commit (with the command lane (A4), that writer ran in this
                // operation's effect window). The plan did not commit, and an
                // operation whose effect already started must never go back
                // to the retry queue: it takes the same `EffectUnknown`
                // classification a lost store guard gets above. Effect-free
                // operations keep the error as-is: the callers' retry paths
                // (the client action loop, the ready pass) own them.
                if effect_started && is_conflict(&error) {
                    if let Err(mark_error) = self.mark_effect_unknown(claimed).await {
                        tracing::warn!(
                            id = ?id,
                            error = %mark_error,
                            "marking an effect-unknown operation failed; its lease expiry will decide it"
                        );
                    }
                    return Err(conflict(format!(
                        "commit conflict after an external effect on operation {id:?}; manual reconciliation required"
                    )));
                }
                return Err(error);
            }
            Ok(
                crate::scheduler::durable::async_document_store::TransitionCommitOutcome::Committed { receipt, loaded }
                | crate::scheduler::durable::async_document_store::TransitionCommitOutcome::AlreadyCommitted { receipt, loaded },
            ) => (receipt, loaded),
            Ok(crate::scheduler::durable::async_document_store::TransitionCommitOutcome::Conflict { .. }) => {
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
        // Core's whole post-commit rule is one value (A6): the plan plus what
        // the committed snapshot means for lifecycle, settlement and the
        // admission slot. Everything below only executes it in the order the
        // durability contracts require.
        let admission = self.admissions.lock().directive(&loaded.snapshot);
        let decision = Decision::committed(plan, &loaded.snapshot, admission);
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
                    .snapshot
                    .operations
                    .get(operation)
                    .is_some_and(|created| created.kind == OperationKind::DispatchTimeout)
            {
                continue;
            }
            self.wake(operation);
        }
        operation_trace::end(operation_trace::WAKE, phase);
        // The read model follows the same commit — the snapshot that commit
        // returned, never a fresh load: a load is a second read that can
        // already be older than a concurrent operation's commit, and projecting it
        // would write that operation's rows away with an older state (the
        // projection's write order must be the commit order). It runs before
        // any external observer sees the commit: the lifecycle events and the
        // outbox transport below.
        let phase = operation_trace::start();
        let projected = match projection {
            Projection::Whole => self.project(&loaded.snapshot).await,
            Projection::Touched => {
                let touched: std::collections::BTreeSet<String> = decision
                    .transition
                    .state_changes
                    .iter()
                    .map(|change| change.task.wire(&loaded.snapshot.process_id))
                    .chain(
                        decision
                            .transition
                            .task_creates
                            .iter()
                            .map(|create| create.task.wire(&loaded.snapshot.process_id)),
                    )
                    .chain(
                        decision
                            .transition
                            .variable_writes
                            .iter()
                            .map(|write| write.scope.wire(&loaded.snapshot.process_id)),
                    )
                    .collect();
                self.project_touched(&loaded.snapshot, &touched).await
            }
        };
        operation_trace::end(operation_trace::PROJECT, phase);
        // A6/A5: an external observer — the lifecycle sink and the outbox
        // transport — sees a commit only after its rows are current. If the
        // projection failed, the run is in the repair queue and this commit's
        // records stay `Pending`: the recovery pass repairs the rows, and the
        // outbox pass projects again before it delivers. This is the strong
        // ordering contract; a reader can therefore never observe a message
        // or completion event whose rows are still behind (except through a
        // direct row read during the failure window, which reports its own
        // lag).
        if projected {
            // The run's lifecycle, from the decision Core made about the
            // freshly committed state: a start while it is alive, one terminal
            // event once every execution settled. A closed sink or an absent
            // one makes this a no-op.
            let phase = operation_trace::start();
            self.report_lifecycle_decision(&loaded.snapshot, decision.lifecycle.as_ref());
            operation_trace::end(operation_trace::LIFECYCLE, phase);
            // The commit's own outbox records are now visible to the
            // transport — after the aggregate commit and after the projection
            // — and their outcomes are persisted in one batch confirmation. A
            // record the confirmation could not settle stays `Pending`; the
            // outbox pass owns the retry.
            let phase = operation_trace::start();
            self.deliver_committed_outbox(&mut loaded, &receipt.new_outbox)
                .await;
            operation_trace::end(operation_trace::DELIVER, phase);
        } else if !receipt.new_outbox.is_empty() {
            // Keep the deliverable work visible to the outbox pass (the
            // records are already `Pending`; the pass re-reads them).
            self.deliver.notify_one();
        }
        // This operation may have settled the run: the slot Core's directive
        // frees goes back to the parked queue right here, so the run that has
        // waited longest starts now instead of on the driver's next tick. The
        // directive was decided from the committed snapshot; the shell only
        // mutates its own set and wakes the admitted operation.
        let phase = operation_trace::start();
        let freed = self
            .admissions
            .lock()
            .apply(&loaded.snapshot, decision.admission);
        if let Some(admission) = freed {
            self.admit_run(&admission).await?;
        }
        operation_trace::end(operation_trace::PLACE2, phase);
        Ok(())
    }
}
