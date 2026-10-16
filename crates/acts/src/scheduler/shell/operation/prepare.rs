//! Phase 1 of an operation: preparing an operation to run.
//!
//! The parked fast path, the slot policy, the kind's pre-claim policy and the
//! claim — everything that must happen before the operation's own work starts,
//! and nothing that depends on the work's outcome. The claim is one atomic
//! `ClaimAndPrepare` event whose commit hands back the authoritative snapshot
//! phases 2 and 3 are formed from.

use super::*;
use crate::scheduler::shell::*;

impl KernelHost {
    /// Phase 1: whether this operation may be driven, and the claimed operation it
    /// drives. `Ok(None)` is the "nothing to do" answer — a run parked by the
    /// slot cap, an operation that already moved past the claimable states, or
    /// a kind whose pre-claim check says there is nothing to do.
    ///
    /// `slot` says whether the run's slot policy applies ([`Slot::Policy`], the
    /// driver's own operations) or the operation is claimed directly
    /// ([`Slot::Direct`], the client action path — see [`Slot`]).
    pub(in crate::scheduler::shell) async fn prepare_operation(
        &self,
        id: &OperationId,
        slot: Slot,
    ) -> Result<Option<Claimed>> {
        // A parked run's operations are re-woken by the recovery scan every
        // tick, so the parked answer comes from the set — a store read would
        // be pure waste on that path.
        if slot == Slot::Policy && self.admissions.lock().is_parked(&id.process_id) {
            return Ok(None);
        }
        let now = SystemClock.now_ms();
        let snapshot = self
            .load(&id.process_id)
            .await?
            .ok_or_else(|| ActError::Runtime(format!("aggregate {} not found", id.process_id)))?
            .snapshot;
        // The slot policy for this run, before it is driven: a parked run
        // waits, a run this driver never admitted (one already in flight when
        // it booted) takes a slot or parks, and a run that settled already —
        // its last operation committed, a client action closed it, or a crash left
        // its timer open — frees its slot for the parked run that has waited
        // longest. The recovery scan re-wakes every pending operation each
        // tick, so this is also the hot path of a parked run.
        if slot == Slot::Policy {
            let placed = self.place_run(&snapshot);
            if !placed {
                return Ok(None);
            }
        }
        let record = snapshot
            .operations
            .get(id)
            .cloned()
            .ok_or_else(|| ActError::Runtime(format!("operation {id:?} not found")))?;
        match record.state {
            OperationState::Pending | OperationState::RetryScheduled { .. } => {}
            _ => return Ok(None),
        }

        // The kind's own pre-claim policy. Today only the timer has one: its
        // operation wakes once per tick and usually has nothing to do, and
        // evaluating its branches before the claim keeps an idle round from
        // consuming the operation (it stays `Pending`, so the next recovery
        // scan re-wakes it — the kernel's tick cadence) and from writing
        // anything. Only the *idle* verdict short-circuits here; the round is
        // decided again after the claim, against the authoritative snapshot.
        if let Some(driver) = driver(record.kind) {
            let pending = Pending {
                id,
                snapshot: &snapshot,
                now_ms: now,
            };
            if matches!(driver.precheck(self, &pending).await?, Precheck::Ignore) {
                return Ok(None);
            }
        }

        // The claim's fence must advance past the operation's own attempt: the
        // state machine refuses a claim that does not (`RetryScheduled{attempt}`
        // and `EffectUnknown{attempt}` require `fence > attempt`), and the
        // driver used to claim with a fixed fence of 1 — so an operation whose claim
        // recovery had released for a retry (`RetryScheduled{attempt: 1}`, the
        // state both a lease expiry and a lost commit produce) could never be
        // claimed again, and its run was stranded for good.
        let attempt = match &record.state {
            OperationState::RetryScheduled { attempt }
            | OperationState::EffectUnknown { attempt } => u64::from(*attempt),
            _ => 0,
        };
        let claim = crate::scheduler::core::model::operation::ClaimToken {
            owner: OWNER.into(),
            fence: attempt + 1,
            lease_until_ms: now + LEASE_MS,
            heartbeat_revision: 0,
        };
        let (prepared, snapshot) = self
            .operation_event(
                id,
                record.revision,
                OperationEvent::ClaimAndPrepare(claim.clone()),
            )
            .await?;
        debug_assert!(matches!(prepared.state, OperationState::CommitPrepared(_)));

        Ok(Some(Claimed {
            id: id.clone(),
            kind: record.kind,
            claim: OperationCommit {
                id: id.clone(),
                claim,
            },
            revision: prepared.revision,
            snapshot,
            now_ms: now,
        }))
    }

    /// Give a claimed-but-unused operation back: its claim is released for the
    /// next tick — the same `RetryScheduled` state a lease expiry produces.
    pub(in crate::scheduler::shell) async fn release_claim(
        &self,
        claimed: &Claimed,
        attempt: u32,
    ) -> Result<()> {
        self.operation_event(
            &claimed.id,
            claimed.revision,
            OperationEvent::ScheduleRetry {
                claim: claimed.claim.claim.clone(),
                attempt,
            },
        )
        .await?;
        Ok(())
    }
}
