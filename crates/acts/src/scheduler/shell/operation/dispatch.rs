//! Phase 4 of a claimed operation: dispatching the plan phase 3 built.
//!
//! One commit is the operation's whole durable effect: the plan's outbox records are
//! delivered first (the at-least-once window the effect seam documents), the
//! transition is committed with the operation's own claim as its source operation —
//! which closes the operation *inside* the same atomic batch — and only then
//! do the operations the commit created get woken, the run's lifecycle
//! reported, the read model re-materialized and the slot bookkeeping settled.

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
    /// Phase 4: deliver, commit, wake and settle. An error means the operation's
    /// effect is *not* durable: a lost compare-and-swap releases the claim and
    /// leaves the operation for recovery, every other failure leaves it open.
    pub(in crate::scheduler::shell) async fn dispatch_operation(
        &self,
        claimed: &Claimed,
        plan: TransitionPlan,
        projection: Projection,
    ) -> Result<()> {
        let id = &claimed.id;
        let mut plan = plan;
        self.deliver_plan(&id.process_id, &mut plan);
        let (receipt, loaded) = match self
            .processes
            .commit_transition(&plan)
            .await
            .map_err(repository_error)?
        {
            crate::scheduler::durable::async_document_store::TransitionCommitOutcome::Committed { receipt, snapshot }
            | crate::scheduler::durable::async_document_store::TransitionCommitOutcome::AlreadyCommitted { receipt, snapshot } => (receipt, snapshot),
            crate::scheduler::durable::async_document_store::TransitionCommitOutcome::Conflict { .. } => {
                // Another writer advanced the aggregate; the wake stays open
                // and recovery re-derives the operation against the new state. A
                // conflict applies nothing, so the plan definitely did not
                // commit: the claim is released back here rather than left
                // `CommitPrepared` until its lease expires — that route ends in
                // `EffectUnknown`, whose only disposition is manual
                // reconciliation, so a claimed operation that merely lost a compare-and-swap
                // would be stranded for good (and the run with it).
                if let Err(error) = self.release_claim(claimed, claimed.claim.claim.fence as u32).await {
                    tracing::warn!(
                        id = ?id,
                        error = %error,
                        "releasing a conflicted operation's claim failed; recovery's lease expiry decides it"
                    );
                }
                // The conflict is the retryable race it is: a caller that
                // planned against a snapshot now stale (the client action
                // loop, above all) must be able to tell it from a refusal.
                return Err(conflict(format!("commit conflict on operation {id:?}")));
            }
        };
        // The commit closed the operation inside its batch (see
        // `advance_transition`): there is no second, close-only commit to run.
        let wake = QueueWake {
            tx: self.wake_tx.clone(),
        };
        for operation in &receipt.new_operations {
            // A timer armed by an execution's *start* operation waits for the tick:
            // the legacy engine had no round before its first process tick, so
            // waking it here would let an already-open window fire up to one
            // tick early. Waking it is the recovery scan's job (it re-wakes
            // every pending operation on each tick). A timer a *round* re-armed
            // (the branch chain of a timed-out execution) is woken at once:
            // that is the legacy `Step::next` handing the turn to the branch
            // declared after the one that just ran.
            if claimed.kind == OperationKind::ExecuteTask
                && loaded
                    .operations
                    .get(operation)
                    .is_some_and(|created| created.kind == OperationKind::DispatchTimeout)
            {
                continue;
            }
            wake.wake(operation);
        }
        // The run's lifecycle, from the freshly committed state: a start while
        // it is alive, one terminal event once every execution settled. A
        // closed sink or an absent one makes this a no-op.
        self.report_lifecycle(&loaded);
        // The read model follows the same commit — the snapshot that commit
        // returned, never a fresh load: a load is a second read that can
        // already be older than a concurrent operation's commit, and projecting it
        // would write that operation's rows away with an older state (the
        // projection's write order must be the commit order).
        match projection {
            Projection::Whole => self.project(&loaded).await,
            Projection::Touched => {
                let touched: std::collections::BTreeSet<String> = plan
                    .state_changes
                    .iter()
                    .map(|change| change.task.node.clone())
                    .chain(
                        plan.task_creates
                            .iter()
                            .map(|create| create.task.node.clone()),
                    )
                    .chain(
                        plan.variable_writes
                            .iter()
                            .map(|write| write.scope.node.clone()),
                    )
                    .collect();
                self.project_touched(&loaded, &touched).await;
            }
        }
        // This operation may have settled the run: its slot goes back to the parked
        // queue right here, so the run that has waited longest starts now
        // instead of on the driver's next tick. The run is the one this operation
        // just drove, so it either holds its slot or needs none — never parked.
        self.place_run(&loaded);
        Ok(())
    }
}
