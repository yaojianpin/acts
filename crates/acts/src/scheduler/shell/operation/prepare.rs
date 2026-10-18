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
    pub(in crate::scheduler::shell) async fn prepare(
        &self,
        id: &OperationId,
        slot: Slot,
    ) -> Result<Option<Claimed>> {
        // A parked run's ready record is deferred by the driver until a slot
        // frees, so the parked answer comes from the set — a store read would
        // be pure waste on the retry path.
        if slot == Slot::Policy && self.admissions.lock().is_parked(&id.process_id) {
            return Ok(None);
        }
        let now = SystemClock.now_ms();
        let phase = operation_trace::start();
        let Some(snapshot) = self.load(&id.process_id).await? else {
            // A ready/recovery/wake record whose aggregate is already gone
            // (an interrupted cleanup, or an older orphan index): there is no
            // work to drive. Skipping keeps the pass alive instead of failing
            // it on a record that cannot become work; the cleanup retry
            // removes the index when its rows still name the process.
            tracing::debug!(
                process_id = %id.process_id,
                "operation record references a missing aggregate; skipping"
            );
            return Ok(None);
        };
        let snapshot = snapshot.snapshot;
        operation_trace::end(operation_trace::LOAD, phase);
        // The slot policy for this run, before it is driven: a parked run
        // waits, a run this driver never admitted (one already in flight when
        // it booted) takes a slot or parks, and a run that settled already —
        // its last operation committed, a client action closed it, or a crash left
        // its timer open — frees its slot for the parked run that has waited
        // longest. A parked run's ready record is deferred by the driver, so
        // this remains the cold path it was.
        if slot == Slot::Policy {
            let phase = operation_trace::start();
            let (placed, admission) = self.place_run(&snapshot);
            operation_trace::end(operation_trace::PLACE1, phase);
            if let Some(admission) = admission {
                self.admit_run(&admission).await?;
            }
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
        // operation is due once per poll and usually has nothing to do, and
        // evaluating its branches before the claim keeps an idle round from
        // consuming the operation (it stays `Pending`; the driver defers its
        // ready record one poll interval) and from writing anything. Only the
        // *idle* verdict short-circuits here; the round is decided again after
        // the claim, against the authoritative snapshot.
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
        let phase = operation_trace::start();
        let (prepared, snapshot) = self
            .operation_event(
                id,
                record.revision,
                OperationEvent::ClaimAndPrepare(claim.clone()),
            )
            .await?;
        operation_trace::end(operation_trace::CLAIM, phase);
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
    ///
    /// A release can itself lose the aggregate compare-and-swap: a client
    /// action or an outbox claim/confirm may be committing the same process
    /// between this operation's load and its release. Retrying the operation
    /// event is safe — the operation revision is unchanged and the event is
    /// the same — and keeps a merely-conflicted operation from sitting
    /// `CommitPrepared` until its lease expires.
    pub(in crate::scheduler::shell) async fn release_claim(
        &self,
        claimed: &Claimed,
        attempt: u32,
    ) -> Result<()> {
        let mut last_error = None;
        for _ in 0..8 {
            match self
                .operation_event(
                    &claimed.id,
                    claimed.revision,
                    OperationEvent::ScheduleRetry {
                        claim: claimed.claim.claim.clone(),
                        attempt,
                    },
                )
                .await
            {
                Ok(_) => return Ok(()),
                Err(error) if is_conflict(&error) => {
                    last_error = Some(error);
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_error.expect("a release retry loop always records its conflict"))
    }
}
