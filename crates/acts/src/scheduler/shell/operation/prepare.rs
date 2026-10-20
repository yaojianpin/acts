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
        self.prepare_with(id, slot, false).await
    }

    /// Claim an `EffectUnknown` operation to replay the executor's observation
    /// (A1c). The decision is already proof, so the slot policy is bypassed
    /// exactly like a client action's; only this path accepts
    /// `EffectUnknown`, and the normal driver never reaches it (an unknown
    /// effect has no ready record and is never woken as normal work).
    pub(in crate::scheduler::shell) async fn prepare_reconciliation(
        &self,
        id: &OperationId,
    ) -> Result<Option<Claimed>> {
        self.prepare_with(id, Slot::Direct, true).await
    }

    async fn prepare_with(
        &self,
        id: &OperationId,
        slot: Slot,
        allow_effect_unknown: bool,
    ) -> Result<Option<Claimed>> {
        // A parked run's ready record is deferred by the driver until a slot
        // frees, so the parked answer comes from the set — a store read would
        // be pure waste on the retry path.
        if slot == Slot::Policy && self.admissions.lock().is_parked(&id.process_id) {
            return Ok(None);
        }
        let now = SystemClock.now_ms();
        // This load is the claim's CAS token too: the pre-claim checks below
        // (`place_run`, the operation state, the kind's precheck) all decide
        // against exactly the documents the claim commit guards, so committing
        // the claim from a second read of the same aggregate would only decode
        // the same bytes again.
        let Some(loaded) = self.load(&id.process_id).await? else {
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
        let snapshot = &loaded.snapshot;
        // The slot policy for this run, before it is driven: a parked run
        // waits, a run this driver never admitted (one already in flight when
        // it booted) takes a slot or parks, and a run that settled already —
        // its last operation committed, a client action closed it, or a crash left
        // its timer open — frees its slot for the parked run that has waited
        // longest. A parked run's ready record is deferred by the driver, so
        // this remains the cold path it was.
        if slot == Slot::Policy {
            let (placed, admission) = self.place_run(snapshot);
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
            OperationState::EffectUnknown { .. } if allow_effect_unknown => {}
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
                snapshot,
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
        let (prepared, loaded) = self
            .operation_event_from(
                loaded,
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
            loaded: Some(loaded),
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

    fn id(sequence: u64) -> OperationId {
        OperationId {
            process_id: "p".into(),
            task: TaskId::new("a", 0).unwrap(),
            sequence,
        }
    }

    fn root_id(sequence: u64) -> OperationId {
        OperationId {
            process_id: "p".into(),
            task: TaskId::new("root", 0).unwrap(),
            sequence,
        }
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

    /// A ready record whose aggregate is gone has no work to drive.
    #[tokio::test]
    async fn a_missing_aggregate_is_skipped() {
        let host = host();
        assert!(
            host.prepare(&id(0), Slot::Direct).await.unwrap().is_none(),
            "no aggregate means no operation"
        );
    }

    /// An operation the aggregate no longer holds is a programming error, not
    /// a silent no-op.
    #[tokio::test]
    async fn a_missing_operation_is_an_error() {
        let host = host();
        persist(&host, &seeded()).await;
        let error = match host.prepare(&id(9), Slot::Direct).await {
            Err(error) => error,
            Ok(_) => panic!("a missing operation must fail the prepare"),
        };
        assert!(format!("{error}").contains("not found"), "{error}");
    }

    /// An operation past the claimable states is not driven again.
    #[tokio::test]
    async fn a_completed_operation_is_not_reclaimed() {
        let host = host();
        let mut snapshot = seeded();
        let op_id = id(0);
        let mut record = OperationRecord::pending(op_id.clone(), OperationKind::ExecuteTask);
        record.state =
            OperationState::Completed(crate::scheduler::core::model::operation::CommitToken {
                process_version: 1,
                digest: "done".into(),
            });
        snapshot.operations.insert(op_id.clone(), record);
        persist(&host, &snapshot).await;
        assert!(host.prepare(&op_id, Slot::Direct).await.unwrap().is_none());
    }

    /// The idle timer preflight leaves the operation `Pending`: nothing is due,
    /// so nothing is claimed.
    #[tokio::test]
    async fn an_idle_timer_is_not_claimed() {
        let host = host();
        let workflow = crate::model::Workflow::new().with_step(|step| {
            step.with_id("s1")
                .with_timeout(|timeout| timeout.with_id("t1"))
        });
        let spec = ProcessSpec::from_workflow("p", &workflow, &|_| None).unwrap();
        let mut snapshot = spec.seed_snapshot().unwrap();
        let owner = TaskId::new("s1", 0).unwrap();
        snapshot.tasks.get_mut(&owner).unwrap().state = TaskState::Running;
        // The declared branch is in flight: the round blocks on it.
        snapshot.tasks.insert(
            TaskId::new("t1", 0).unwrap(),
            crate::scheduler::core::commit::aggregate::TaskSnapshot {
                model_node: crate::scheduler::core::model::model_graph::NodeId::parse("t1")
                    .unwrap(),
                parent: Some(owner.clone()),
                predecessors: vec![owner.clone()],
                state: TaskState::Running,
                revision: 0,
            },
        );
        let op_id = OperationId {
            process_id: "p".into(),
            task: owner,
            sequence: 0,
        };
        snapshot.operations.insert(
            op_id.clone(),
            OperationRecord::pending(op_id.clone(), OperationKind::DispatchTimeout),
        );
        persist(&host, &snapshot).await;
        assert!(host.prepare(&op_id, Slot::Direct).await.unwrap().is_none());
    }

    /// A release that guards the pre-claim revision is refused at once: the
    /// operation revision moved, and that is not the replace-level CAS the
    /// loop retries.
    #[tokio::test]
    async fn a_stale_release_is_refused() {
        let host = host();
        persist(&host, &seeded()).await;
        let mut claimed = host
            .prepare(&root_id(0), Slot::Direct)
            .await
            .unwrap()
            .expect("the seed operation is claimable");
        assert!(claimed.revision > 0);
        // The claim committed; a release that guards the pre-claim revision
        // can never take.
        claimed.revision = 0;
        let error = host.release_claim(&claimed, 1).await.unwrap_err();
        assert!(
            !is_conflict(&error),
            "a stale revision is a refusal: {error}"
        );
    }

    /// A release the state machine refuses is not a conflict and is surfaced
    /// at once.
    #[tokio::test]
    async fn a_refused_release_surfaces_the_domain_error() {
        let host = host();
        persist(&host, &seeded()).await;
        let mut claimed = host
            .prepare(&root_id(0), Slot::Direct)
            .await
            .unwrap()
            .expect("the seed operation is claimable");
        // The fence no longer matches the claim the record carries.
        claimed.claim.claim.fence = 99;
        let error = host.release_claim(&claimed, 1).await.unwrap_err();
        assert!(!is_conflict(&error), "a refusal is not a lost CAS: {error}");
    }
}
