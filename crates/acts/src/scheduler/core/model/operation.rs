use super::task_graph::TaskId;
use std::fmt;

/// Stable identity of one durable unit of work.
///
/// `task` is the instance the work is attached to; `sequence` numbers the
/// work items of that one instance.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct OperationId {
    pub(crate) process_id: String,
    pub(crate) task: TaskId,
    pub(crate) sequence: u64,
}

impl fmt::Debug for OperationId {
    /// Renders the identity flat — process, node, generation, sequence — so
    /// diagnostics that interpolate an operation (`operation {id:?} not
    /// found`) keep their historical byte shape.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OperationId")
            .field("process_id", &self.process_id)
            .field("node", &self.task.node.as_str())
            .field("generation", &self.task.generation)
            .field("sequence", &self.sequence)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OperationKind {
    ExecuteTask,
    PropagateTask,
    ApplyAction,
    /// Cascade a cancellation/failure past a created execution: the operation
    /// commits `Ready → Skipped` and hands propagation to a `PropagateTask`
    /// operation, whose readiness then treats the skipped source as dead.
    SkipTask,
    /// The abort cascade's operation for a `Running` successor of an aborted
    /// execution: the legacy ancestor walk marks running children `Aborted`
    /// (pending ones skip). Like `SkipTask` it is one atomic
    /// `Running → Aborted` commit plus a `PropagateTask` hand-off.
    AbortTask,
    /// The timer of an execution whose node declares `timeouts`: while that
    /// execution is `Running`/`Waiting`, the tick evaluates its declared
    /// branches in declaration order (the one-shot `__timeouts` marker, the
    /// in-flight check) and admits the first due one. The operation re-arms
    /// itself with a fresh intent after every dispatch and stays `Pending`
    /// while nothing is due; its ready record is deferred by one poll interval,
    /// the kernel's dispatch cadence (the legacy process tick's equivalent), and
    /// it retires once the execution is decided or every branch fired.
    DispatchTimeout,
    /// The durable rewind a `Cancel`/`Back` decision leaves behind: the
    /// deciding commit closes the source generation (`Backed`) and creates
    /// this operation in the *same* atomic batch, so the re-entry cannot be
    /// lost to a crash or a lost compare-and-swap — its operation plans the fresh
    /// generations ([`crate::scheduler::core::plan::generation::Reenter`]) and its
    /// ready record re-drives it if that operation fails. The target task id is
    /// the durable fact `__reenter_target` the deciding commit writes (`Back`
    /// names an earlier step, `Cancel` the closed execution itself).
    ReenterTask,
    /// A subflow child's hand-back: once the child run settled, the operation
    /// returns its outcome (outputs and terminal state) to the parent act that
    /// started it, completing/failing/skipping/aborting that act. The child
    /// retains the operation until the parent decision is committed, so the
    /// return survives a crash between the child's terminal commit and the
    /// parent's action.
    ReturnToParent,
    DeliverMessage,
    FireTrigger,
    RunPostCommit,
}

impl OperationKind {
    /// Whether this kind is normal, driven work and therefore belongs in the
    /// durable ready index.
    ///
    /// The undriven vocabulary kinds (`DeliverMessage`, `FireTrigger`,
    /// `RunPostCommit`) never had a host driver, so indexing them would only
    /// make the due pass report the same "not driven yet" error forever. The
    /// list sits beside the enum (A6) so a new kind is classified where it is
    /// declared instead of in the ready/queue plumbing.
    pub(crate) fn is_schedulable(self) -> bool {
        matches!(
            self,
            Self::ExecuteTask
                | Self::ApplyAction
                | Self::SkipTask
                | Self::AbortTask
                | Self::DispatchTimeout
                | Self::PropagateTask
                | Self::ReenterTask
                | Self::ReturnToParent
        )
    }
}

/// A persisted operation has one authoritative lifecycle.
///
/// `Completed` is intentionally unreachable from `Pending` or `Claimed`.
/// Business state must first be atomically committed and identified by a
/// commit token. This removes the old ambiguity where "completed" could mean
/// merely dequeued, locally handled, or durably applied.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OperationState {
    Pending,
    Claimed(ClaimToken),
    CommitPrepared(ClaimToken),
    Committed(CommitToken),
    RetryScheduled { attempt: u32 },
    EffectUnknown { attempt: u32 },
    Quarantined { reason: String },
    Superseded { by_generation: u64 },
    Completed(CommitToken),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClaimToken {
    pub(crate) owner: String,
    pub(crate) fence: u64,
    pub(crate) lease_until_ms: i64,
    pub(crate) heartbeat_revision: u64,
}

impl ClaimToken {
    pub(crate) fn is_expired(&self, now_ms: i64) -> bool {
        now_ms >= self.lease_until_ms
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CommitToken {
    pub(crate) process_version: u64,
    pub(crate) digest: String,
}

/// The durable lifecycle events one operation accepts.
///
/// `MarkEffectUnknown` is emitted at the external-effect boundary (A1a), and
/// `ResolveEffectNotApplied` is emitted by recovery when an executor's
/// observation proves the effect did not run and the effect is idempotent
/// (A1c). `RenewClaim`, `Quarantine`, `ReleaseQuarantine` and `Supersede` are
/// transitions of the operation lifecycle recovery classifies and
/// `snapshot_codec` persists (the states they produce are part of the durable
/// V2 format, tags 5–7), but no in-tree caller emits them today. Kept as the
/// lifecycle's stated vocabulary rather than deleted, because deleting the
/// events would delete the states they produce and change the persisted
/// format.
#[allow(dead_code)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OperationEvent {
    Claim(ClaimToken),
    /// Take ownership and mark the commit prepared in one durable step.
    /// Nothing observable happens between the two for the single owner that
    /// drives one, so the pair is a needless second round trip.
    ClaimAndPrepare(ClaimToken),
    RenewClaim {
        current: ClaimToken,
        renewed: ClaimToken,
    },
    PrepareCommit {
        claim: ClaimToken,
    },
    Commit {
        claim: ClaimToken,
        commit: CommitToken,
    },
    Complete {
        commit: CommitToken,
    },
    ScheduleRetry {
        claim: ClaimToken,
        attempt: u32,
    },
    MarkEffectUnknown {
        claim: ClaimToken,
        attempt: u32,
    },
    /// An operator/executor observation proved that the effect of an
    /// `EffectUnknown` operation never ran. Only a provably idempotent effect
    /// may take this path automatically; the record returns to the retry
    /// queue under the same operation identity (and therefore the same effect
    /// key).
    ResolveEffectNotApplied {
        attempt: u32,
    },
    Quarantine {
        claim: ClaimToken,
        reason: String,
    },
    /// Explicit operator compensation. The next attempt must advance beyond
    /// the quarantined generation; automatic recovery never emits this event.
    ReleaseQuarantine {
        next_attempt: u32,
    },
    ReleaseExpiredClaim {
        now_ms: i64,
    },
    Supersede {
        by_generation: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OperationRecord {
    pub(crate) id: OperationId,
    pub(crate) kind: OperationKind,
    pub(crate) state: OperationState,
    pub(crate) revision: u64,
}

fn valid_initial_claim(claim: &ClaimToken) -> bool {
    !claim.owner.trim().is_empty()
        && (1..=u64::from(u32::MAX)).contains(&claim.fence)
        && claim.lease_until_ms != i64::MIN
        && claim.heartbeat_revision == 0
}

impl OperationRecord {
    pub(crate) fn pending(id: OperationId, kind: OperationKind) -> Self {
        Self {
            id,
            kind,
            state: OperationState::Pending,
            revision: 0,
        }
    }

    /// Pure transition. Persistence layers apply the returned record with a
    /// compare-and-swap on `revision`; queues only wake consumers and never
    /// mutate this state directly.
    pub(crate) fn evolve(&self, event: OperationEvent) -> Result<Self, OperationTransitionError> {
        use OperationEvent as E;
        use OperationState as S;

        let next = match (&self.state, event) {
            (S::Pending, E::Claim(claim)) if valid_initial_claim(&claim) => S::Claimed(claim),
            (S::RetryScheduled { attempt }, E::Claim(claim))
            | (S::EffectUnknown { attempt }, E::Claim(claim))
                if valid_initial_claim(&claim) && claim.fence > u64::from(*attempt) =>
            {
                S::Claimed(claim)
            }

            // Ownership and the prepared commit are one transition: the operation
            // that owns an operation does both with nothing in between that
            // another owner could observe, so one durable step says exactly
            // what two did. A crash after it is the `CommitPrepared` case the
            // recovery scan already handles (its lease expires, the owner is
            // reclaimed and the operation re-runs).
            (S::Pending, E::ClaimAndPrepare(claim)) if valid_initial_claim(&claim) => {
                S::CommitPrepared(claim)
            }
            (S::RetryScheduled { attempt }, E::ClaimAndPrepare(claim))
            | (S::EffectUnknown { attempt }, E::ClaimAndPrepare(claim))
                if valid_initial_claim(&claim) && claim.fence > u64::from(*attempt) =>
            {
                S::CommitPrepared(claim)
            }

            (
                S::Claimed(current),
                E::RenewClaim {
                    current: expected,
                    renewed,
                },
            )
            | (
                S::CommitPrepared(current),
                E::RenewClaim {
                    current: expected,
                    renewed,
                },
            ) if current == &expected
                && renewed.owner == current.owner
                && renewed.fence == current.fence
                && current.heartbeat_revision.checked_add(1)
                    == Some(renewed.heartbeat_revision)
                && renewed.lease_until_ms > current.lease_until_ms =>
            {
                match self.state {
                    S::Claimed(_) => S::Claimed(renewed),
                    S::CommitPrepared(_) => S::CommitPrepared(renewed),
                    _ => unreachable!(),
                }
            }

            (S::Claimed(current), E::PrepareCommit { claim }) if current == &claim => {
                S::CommitPrepared(claim)
            }

            (S::CommitPrepared(current), E::Commit { claim, commit }) if current == &claim => {
                S::Committed(commit)
            }

            (S::Committed(current), E::Complete { commit }) if current == &commit => {
                S::Completed(commit)
            }

            (S::Claimed(current), E::ScheduleRetry { claim, attempt })
            | (S::CommitPrepared(current), E::ScheduleRetry { claim, attempt })
                if current == &claim && attempt > 0 && u64::from(attempt) == current.fence =>
            {
                S::RetryScheduled { attempt }
            }

            (S::Claimed(current), E::MarkEffectUnknown { claim, attempt })
            | (S::CommitPrepared(current), E::MarkEffectUnknown { claim, attempt })
                if current == &claim && attempt > 0 && u64::from(attempt) == current.fence =>
            {
                S::EffectUnknown { attempt }
            }

            // The proof that the effect never ran clears the unknown state
            // without lowering the attempt: the next claim is fenced at
            // `attempt + 1`, exactly like a lease-expiry retry.
            (S::EffectUnknown { attempt: current }, E::ResolveEffectNotApplied { attempt })
                if current == &attempt && attempt > 0 =>
            {
                S::RetryScheduled { attempt }
            }

            // Claimed means the handler has not crossed the effect boundary yet,
            // so an expired owner can be retried safely. CommitPrepared is the
            // execution boundary: after it is durable, a crash may have occurred
            // after the external effect but before its outcome was recorded.
            (S::Claimed(claim), E::ReleaseExpiredClaim { now_ms })
                if claim.is_expired(now_ms) && claim.fence <= u64::from(u32::MAX) =>
            {
                S::RetryScheduled {
                    attempt: claim.fence as u32,
                }
            }
            (S::CommitPrepared(claim), E::ReleaseExpiredClaim { now_ms })
                if claim.is_expired(now_ms) && claim.fence <= u64::from(u32::MAX) =>
            {
                S::EffectUnknown {
                    attempt: claim.fence as u32,
                }
            }

            (S::Claimed(current), E::Quarantine { claim, reason })
            | (S::CommitPrepared(current), E::Quarantine { claim, reason })
                if current == &claim && !reason.trim().is_empty() =>
            {
                S::Quarantined { reason }
            }

            (S::Quarantined { .. }, E::ReleaseQuarantine { next_attempt }) if next_attempt > 0 => {
                S::RetryScheduled {
                    attempt: next_attempt,
                }
            }

            (S::Pending, E::Supersede { by_generation })
            | (S::RetryScheduled { .. }, E::Supersede { by_generation })
            | (S::EffectUnknown { .. }, E::Supersede { by_generation })
                if by_generation > self.id.task.generation =>
            {
                S::Superseded { by_generation }
            }

            (_, event) => {
                return Err(OperationTransitionError::InvalidTransition {
                    from: self.state.clone(),
                    event,
                });
            }
        };

        Ok(Self {
            id: self.id.clone(),
            kind: self.kind,
            state: next,
            revision: self
                .revision
                .checked_add(1)
                .ok_or(OperationTransitionError::RevisionOverflow)?,
        })
    }

    pub(crate) fn is_terminal(&self) -> bool {
        matches!(
            self.state,
            OperationState::Completed(_)
                | OperationState::Quarantined { .. }
                | OperationState::Superseded { .. }
        )
    }

    /// Whether this record belongs to the recovery scan.
    ///
    /// Normal runnable work (`Pending`, `RetryScheduled`) has its own durable
    /// ready index and is never re-woken by recovery. Recovery's index carries
    /// only the states that need an external decision: an owner that may have
    /// died, a commit waiting to be finalized, or an operation whose effect
    /// outcome must be reconciled by a human. `Quarantined` is terminal for
    /// execution but stays here precisely so it keeps being surfaced.
    pub(crate) fn needs_recovery(&self) -> bool {
        matches!(
            self.state,
            OperationState::Claimed(_)
                | OperationState::CommitPrepared(_)
                | OperationState::Committed(_)
                | OperationState::EffectUnknown { .. }
                | OperationState::Quarantined { .. }
        )
    }

    /// Whether execution has permanently moved past this record.
    pub(crate) fn is_retired(&self) -> bool {
        matches!(
            self.state,
            OperationState::Completed(_) | OperationState::Superseded { .. }
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OperationTransitionError {
    InvalidTransition {
        from: OperationState,
        event: OperationEvent,
    },
    RevisionOverflow,
}

impl fmt::Display for OperationTransitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTransition { from, event } => {
                write!(
                    f,
                    "invalid operation transition from {from:?} via {event:?}"
                )
            }
            Self::RevisionOverflow => write!(f, "operation revision overflow"),
        }
    }
}

impl std::error::Error for OperationTransitionError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> OperationId {
        OperationId {
            process_id: "p1".into(),
            task: TaskId::new("t1", 3).unwrap(),
            sequence: 9,
        }
    }

    fn claim() -> ClaimToken {
        ClaimToken {
            owner: "runtime-a".into(),
            fence: 7,
            lease_until_ms: i64::MAX,
            heartbeat_revision: 0,
        }
    }

    fn commit() -> CommitToken {
        CommitToken {
            process_version: 11,
            digest: "digest".into(),
        }
    }

    #[test]
    fn pending_cannot_complete_directly() {
        let op = OperationRecord::pending(id(), OperationKind::ExecuteTask);
        assert!(
            op.evolve(OperationEvent::Complete { commit: commit() })
                .is_err()
        );
    }

    #[test]
    fn claim_cannot_complete_without_commit() {
        let op = OperationRecord::pending(id(), OperationKind::ExecuteTask)
            .evolve(OperationEvent::Claim(claim()))
            .unwrap();
        assert!(
            op.evolve(OperationEvent::Complete { commit: commit() })
                .is_err()
        );
    }

    #[test]
    fn happy_path_requires_claim_prepare_commit_and_complete() {
        let claim = claim();
        let commit = commit();
        let op = OperationRecord::pending(id(), OperationKind::ApplyAction)
            .evolve(OperationEvent::Claim(claim.clone()))
            .unwrap()
            .evolve(OperationEvent::PrepareCommit {
                claim: claim.clone(),
            })
            .unwrap()
            .evolve(OperationEvent::Commit {
                claim,
                commit: commit.clone(),
            })
            .unwrap()
            .evolve(OperationEvent::Complete {
                commit: commit.clone(),
            })
            .unwrap();
        assert_eq!(op.state, OperationState::Completed(commit));
        assert_eq!(op.revision, 4);
        assert!(op.is_terminal());
    }

    #[test]
    fn stale_claim_cannot_prepare_or_commit() {
        let current = claim();
        let stale = ClaimToken {
            owner: "runtime-b".into(),
            fence: 6,
            lease_until_ms: i64::MAX,
            heartbeat_revision: 0,
        };
        let claimed = OperationRecord::pending(id(), OperationKind::ExecuteTask)
            .evolve(OperationEvent::Claim(current))
            .unwrap();
        assert!(
            claimed
                .evolve(OperationEvent::PrepareCommit { claim: stale })
                .is_err()
        );
    }

    #[test]
    fn supersede_requires_newer_generation() {
        let op = OperationRecord::pending(id(), OperationKind::PropagateTask);
        assert!(
            op.clone()
                .evolve(OperationEvent::Supersede { by_generation: 3 })
                .is_err()
        );
        let op = op
            .evolve(OperationEvent::Supersede { by_generation: 4 })
            .unwrap();
        assert_eq!(op.state, OperationState::Superseded { by_generation: 4 });
    }
    #[test]
    fn expired_claim_can_be_released_but_active_claim_cannot() {
        let expired = ClaimToken {
            owner: "runtime-a".into(),
            fence: 8,
            lease_until_ms: 100,
            heartbeat_revision: 0,
        };
        let claimed = OperationRecord::pending(id(), OperationKind::ExecuteTask)
            .evolve(OperationEvent::Claim(expired))
            .unwrap();
        assert!(
            claimed
                .evolve(OperationEvent::ReleaseExpiredClaim { now_ms: 99 })
                .is_err()
        );
        let released = claimed
            .evolve(OperationEvent::ReleaseExpiredClaim { now_ms: 100 })
            .unwrap();
        assert_eq!(
            released.state,
            OperationState::RetryScheduled { attempt: 8 }
        );
    }

    #[test]
    fn claim_lease_expiry_boundary_is_inclusive() {
        let claim = ClaimToken {
            owner: "runtime-a".into(),
            fence: 9,
            lease_until_ms: 500,
            heartbeat_revision: 4,
        };
        assert!(!claim.is_expired(499));
        assert!(claim.is_expired(500));
    }

    #[test]
    fn heartbeat_must_advance_revision_and_extend_deadline() {
        let current = ClaimToken {
            owner: "runtime-a".into(),
            fence: 7,
            lease_until_ms: 100,
            heartbeat_revision: 0,
        };
        let claimed = OperationRecord::pending(id(), OperationKind::ExecuteTask)
            .evolve(OperationEvent::Claim(current.clone()))
            .unwrap();
        let renewed = ClaimToken {
            owner: current.owner.clone(),
            fence: current.fence,
            lease_until_ms: 200,
            heartbeat_revision: 1,
        };
        assert_eq!(
            claimed
                .evolve(OperationEvent::RenewClaim {
                    current: current.clone(),
                    renewed: renewed.clone()
                })
                .unwrap()
                .state,
            OperationState::Claimed(renewed),
        );
        let stale_revision = ClaimToken {
            heartbeat_revision: 0,
            lease_until_ms: 300,
            ..current.clone()
        };
        assert!(
            claimed
                .evolve(OperationEvent::RenewClaim {
                    current: current.clone(),
                    renewed: stale_revision
                })
                .is_err()
        );
        let shorter = ClaimToken {
            heartbeat_revision: 1,
            lease_until_ms: 99,
            ..current.clone()
        };
        assert!(
            claimed
                .evolve(OperationEvent::RenewClaim {
                    current,
                    renewed: shorter
                })
                .is_err()
        );
    }
    #[test]
    fn revision_overflow_is_a_structured_error_and_preserves_source() {
        let operation = OperationRecord {
            id: OperationId {
                process_id: "p".into(),
                task: TaskId::new("t", 0).unwrap(),
                sequence: 0,
            },
            kind: OperationKind::ExecuteTask,
            state: OperationState::Pending,
            revision: u64::MAX,
        };
        let claim = ClaimToken {
            owner: "worker".into(),
            fence: 1,
            lease_until_ms: 1,
            heartbeat_revision: 0,
        };
        assert_eq!(
            operation.evolve(OperationEvent::Claim(claim)),
            Err(OperationTransitionError::RevisionOverflow)
        );
        assert_eq!(operation.revision, u64::MAX);
        assert_eq!(operation.state, OperationState::Pending);
    }
    #[test]
    fn reclaim_preserves_attempt_and_rejects_stale_fence() {
        let expired = ClaimToken {
            owner: "runtime-a".into(),
            fence: 8,
            lease_until_ms: 100,
            heartbeat_revision: 0,
        };
        let released = OperationRecord::pending(id(), OperationKind::ExecuteTask)
            .evolve(OperationEvent::Claim(expired))
            .unwrap()
            .evolve(OperationEvent::ReleaseExpiredClaim { now_ms: 100 })
            .unwrap();
        assert_eq!(
            released.state,
            OperationState::RetryScheduled { attempt: 8 }
        );

        let stale = ClaimToken {
            owner: "runtime-b".into(),
            fence: 8,
            lease_until_ms: 200,
            heartbeat_revision: 0,
        };
        assert!(released.evolve(OperationEvent::Claim(stale)).is_err());

        let next = ClaimToken {
            owner: "runtime-b".into(),
            fence: 9,
            lease_until_ms: 200,
            heartbeat_revision: 0,
        };
        assert!(released.evolve(OperationEvent::Claim(next)).is_ok());
    }

    #[test]
    fn retry_attempt_must_match_claim_fence() {
        let claim = claim();
        let claimed = OperationRecord::pending(id(), OperationKind::ExecuteTask)
            .evolve(OperationEvent::Claim(claim.clone()))
            .unwrap();
        assert!(
            claimed
                .clone()
                .evolve(OperationEvent::ScheduleRetry {
                    claim: claim.clone(),
                    attempt: 6,
                })
                .is_err()
        );
        assert!(
            claimed
                .evolve(OperationEvent::ScheduleRetry { claim, attempt: 7 })
                .is_ok()
        );
    }

    #[test]
    fn claim_requires_owner_positive_fence_and_fresh_heartbeat() {
        let pending = OperationRecord::pending(id(), OperationKind::ExecuteTask);
        for invalid in [
            ClaimToken {
                owner: "".into(),
                ..claim()
            },
            ClaimToken {
                fence: 0,
                ..claim()
            },
            ClaimToken {
                heartbeat_revision: 1,
                ..claim()
            },
        ] {
            assert!(
                pending
                    .clone()
                    .evolve(OperationEvent::Claim(invalid))
                    .is_err()
            );
        }
    }
    #[test]
    fn expired_prepared_claim_requires_reconciliation_instead_of_retry() {
        let claim = ClaimToken {
            owner: "runtime-a".into(),
            fence: 11,
            lease_until_ms: 100,
            heartbeat_revision: 0,
        };
        let recovered = OperationRecord::pending(id(), OperationKind::ExecuteTask)
            .evolve(OperationEvent::Claim(claim.clone()))
            .unwrap()
            .evolve(OperationEvent::PrepareCommit { claim })
            .unwrap()
            .evolve(OperationEvent::ReleaseExpiredClaim { now_ms: 100 })
            .unwrap();
        assert_eq!(
            recovered.state,
            OperationState::EffectUnknown { attempt: 11 }
        );
    }
    #[test]
    fn terminal_states_reject_claim_without_mutating_the_source_record() {
        let terminal_states = [
            OperationState::Quarantined {
                reason: "bad".into(),
            },
            OperationState::Superseded { by_generation: 4 },
            OperationState::Completed(commit()),
        ];
        for state in terminal_states {
            let source = OperationRecord {
                id: id(),
                kind: OperationKind::ExecuteTask,
                state,
                revision: 9,
            };
            let preserved = source.clone();
            assert!(
                source
                    .clone()
                    .evolve(OperationEvent::Claim(claim()))
                    .is_err()
            );
            assert_eq!(source, preserved);
        }
    }

    #[test]
    fn expired_claimed_and_prepared_states_have_distinct_effect_safety() {
        let claim = ClaimToken {
            owner: "runtime-a".into(),
            fence: 12,
            lease_until_ms: 100,
            heartbeat_revision: 0,
        };
        let claimed = OperationRecord::pending(id(), OperationKind::ExecuteTask)
            .evolve(OperationEvent::Claim(claim.clone()))
            .unwrap();
        let prepared = claimed
            .clone()
            .evolve(OperationEvent::PrepareCommit {
                claim: claim.clone(),
            })
            .unwrap();
        assert_eq!(
            claimed
                .evolve(OperationEvent::ReleaseExpiredClaim { now_ms: 100 })
                .unwrap()
                .state,
            OperationState::RetryScheduled { attempt: 12 }
        );
        assert_eq!(
            prepared
                .evolve(OperationEvent::ReleaseExpiredClaim { now_ms: 100 })
                .unwrap()
                .state,
            OperationState::EffectUnknown { attempt: 12 }
        );
    }
    #[test]
    fn initial_claim_rejects_unpersistable_fence_and_invalid_lease() {
        for invalid in [
            ClaimToken {
                fence: u64::from(u32::MAX) + 1,
                ..claim()
            },
            ClaimToken {
                lease_until_ms: i64::MIN,
                ..claim()
            },
        ] {
            assert!(
                OperationRecord::pending(id(), OperationKind::ExecuteTask)
                    .evolve(OperationEvent::Claim(invalid))
                    .is_err()
            );
        }
    }

    #[test]
    fn heartbeat_overflow_is_rejected() {
        let current = ClaimToken {
            lease_until_ms: 100,
            heartbeat_revision: u64::MAX,
            ..claim()
        };
        let renewed = ClaimToken {
            lease_until_ms: current.lease_until_ms + 1,
            ..current.clone()
        };
        let operation = OperationRecord {
            id: id(),
            kind: OperationKind::ExecuteTask,
            state: OperationState::Claimed(current.clone()),
            revision: 1,
        };
        assert!(
            operation
                .evolve(OperationEvent::RenewClaim { current, renewed })
                .is_err()
        );
    }
    #[test]
    fn a_proven_not_applied_effect_returns_to_the_retry_queue() {
        let claim = ClaimToken {
            owner: "runtime-a".into(),
            fence: 3,
            lease_until_ms: i64::MAX,
            heartbeat_revision: 0,
        };
        let unknown = OperationRecord::pending(id(), OperationKind::ApplyAction)
            .evolve(OperationEvent::ClaimAndPrepare(claim.clone()))
            .unwrap()
            .evolve(OperationEvent::MarkEffectUnknown {
                claim: claim.clone(),
                attempt: 3,
            })
            .unwrap();
        assert_eq!(unknown.state, OperationState::EffectUnknown { attempt: 3 });
        assert!(
            unknown
                .clone()
                .evolve(OperationEvent::ResolveEffectNotApplied { attempt: 2 })
                .is_err(),
            "only the unknown attempt may be resolved"
        );
        let retried = unknown
            .evolve(OperationEvent::ResolveEffectNotApplied { attempt: 3 })
            .unwrap();
        assert_eq!(retried.state, OperationState::RetryScheduled { attempt: 3 });
        // The next claim is fenced past the attempt, exactly like a lease
        // expiry; the operation identity (and its effect key) is unchanged.
        assert!(retried.evolve(OperationEvent::Claim(claim)).is_err());
        assert!(
            retried
                .evolve(OperationEvent::Claim(ClaimToken {
                    owner: "runtime-a".into(),
                    fence: 4,
                    lease_until_ms: i64::MAX,
                    heartbeat_revision: 0,
                }))
                .is_ok()
        );
    }

    #[test]
    fn quarantine_requires_current_claim_and_explicit_compensation() {
        let current = claim();
        let claimed = OperationRecord::pending(id(), OperationKind::ExecuteTask)
            .evolve(OperationEvent::Claim(current.clone()))
            .unwrap();
        let stale = ClaimToken {
            fence: current.fence + 1,
            ..current.clone()
        };
        assert!(
            claimed
                .clone()
                .evolve(OperationEvent::Quarantine {
                    claim: stale,
                    reason: "invalid external contract".into(),
                })
                .is_err()
        );
        let quarantined = claimed
            .evolve(OperationEvent::Quarantine {
                claim: current,
                reason: "invalid external contract".into(),
            })
            .unwrap();
        assert!(
            quarantined
                .clone()
                .evolve(OperationEvent::Claim(claim()))
                .is_err()
        );
        assert_eq!(
            quarantined
                .evolve(OperationEvent::ReleaseQuarantine { next_attempt: 8 })
                .unwrap()
                .state,
            OperationState::RetryScheduled { attempt: 8 }
        );
    }

    #[test]
    fn every_operation_kind_is_classified_where_it_is_declared() {
        for (kind, schedulable) in [
            (OperationKind::ExecuteTask, true),
            (OperationKind::PropagateTask, true),
            (OperationKind::ApplyAction, true),
            (OperationKind::SkipTask, true),
            (OperationKind::AbortTask, true),
            (OperationKind::DispatchTimeout, true),
            (OperationKind::ReenterTask, true),
            (OperationKind::ReturnToParent, true),
            (OperationKind::DeliverMessage, false),
            (OperationKind::FireTrigger, false),
            (OperationKind::RunPostCommit, false),
        ] {
            assert_eq!(kind.is_schedulable(), schedulable, "{kind:?}");
        }
    }

    #[test]
    fn an_unknown_effect_can_be_claimed_under_the_next_fence() {
        let unknown = OperationRecord {
            id: id(),
            kind: OperationKind::ApplyAction,
            state: OperationState::EffectUnknown { attempt: 3 },
            revision: 1,
        };
        assert!(
            unknown
                .clone()
                .evolve(OperationEvent::Claim(ClaimToken {
                    fence: 3,
                    ..claim()
                }))
                .is_err(),
            "the claim must advance past the unknown attempt"
        );
        let claimed = unknown
            .evolve(OperationEvent::ClaimAndPrepare(ClaimToken {
                fence: 4,
                ..claim()
            }))
            .unwrap();
        assert_eq!(
            claimed.state,
            OperationState::CommitPrepared(ClaimToken {
                fence: 4,
                ..claim()
            })
        );
    }

    #[test]
    fn a_prepared_claim_renews_and_quarantines() {
        let current = ClaimToken {
            owner: "runtime-a".into(),
            fence: 7,
            lease_until_ms: 100,
            heartbeat_revision: 0,
        };
        let prepared = OperationRecord {
            id: id(),
            kind: OperationKind::ExecuteTask,
            state: OperationState::CommitPrepared(current.clone()),
            revision: 1,
        };
        let renewed = ClaimToken {
            lease_until_ms: 200,
            heartbeat_revision: 1,
            ..current.clone()
        };
        assert_eq!(
            prepared
                .clone()
                .evolve(OperationEvent::RenewClaim {
                    current: current.clone(),
                    renewed,
                })
                .unwrap()
                .state,
            OperationState::CommitPrepared(ClaimToken {
                lease_until_ms: 200,
                heartbeat_revision: 1,
                ..current.clone()
            })
        );
        assert_eq!(
            prepared
                .evolve(OperationEvent::Quarantine {
                    claim: current,
                    reason: "operator".into(),
                })
                .unwrap()
                .state,
            OperationState::Quarantined {
                reason: "operator".into()
            }
        );
    }

    #[test]
    fn supersede_accepts_every_unstarted_state_but_only_a_newer_generation() {
        for state in [
            OperationState::RetryScheduled { attempt: 2 },
            OperationState::EffectUnknown { attempt: 2 },
        ] {
            let operation = OperationRecord {
                id: id(),
                kind: OperationKind::ExecuteTask,
                state,
                revision: 0,
            };
            assert!(
                operation
                    .clone()
                    .evolve(OperationEvent::Supersede { by_generation: 3 })
                    .is_err(),
                "an equal generation must not supersede"
            );
            assert_eq!(
                operation
                    .evolve(OperationEvent::Supersede { by_generation: 4 })
                    .unwrap()
                    .state,
                OperationState::Superseded { by_generation: 4 }
            );
        }
    }

    #[test]
    fn transition_errors_render_both_variants() {
        let invalid = OperationTransitionError::InvalidTransition {
            from: OperationState::Pending,
            event: OperationEvent::Complete { commit: commit() },
        };
        assert!(invalid.to_string().contains("invalid operation transition"));
        assert_eq!(
            OperationTransitionError::RevisionOverflow.to_string(),
            "operation revision overflow"
        );
    }
}

#[cfg(test)]
mod branch_tests {
    use super::*;

    fn id() -> OperationId {
        OperationId {
            process_id: "p1".into(),
            task: TaskId::new("t1", 3).unwrap(),
            sequence: 9,
        }
    }

    fn claim() -> ClaimToken {
        ClaimToken {
            owner: "runtime-a".into(),
            fence: 7,
            lease_until_ms: i64::MAX,
            heartbeat_revision: 0,
        }
    }

    fn commit() -> CommitToken {
        CommitToken {
            process_version: 11,
            digest: "digest".into(),
        }
    }

    #[test]
    fn claim_and_prepare_requires_a_valid_fresh_claim() {
        let pending = OperationRecord::pending(id(), OperationKind::ExecuteTask);
        for invalid in [
            ClaimToken {
                owner: " ".into(),
                ..claim()
            },
            ClaimToken {
                fence: 0,
                ..claim()
            },
        ] {
            assert!(
                pending
                    .clone()
                    .evolve(OperationEvent::ClaimAndPrepare(invalid))
                    .is_err(),
                "the guard rejects an unpersistable claim"
            );
        }
        let retried = OperationRecord {
            id: id(),
            kind: OperationKind::ExecuteTask,
            state: OperationState::RetryScheduled { attempt: 5 },
            revision: 1,
        };
        assert!(
            retried
                .clone()
                .evolve(OperationEvent::ClaimAndPrepare(ClaimToken {
                    fence: 5,
                    ..claim()
                }))
                .is_err(),
            "the fence must advance past the attempt"
        );
        assert_eq!(
            retried
                .evolve(OperationEvent::ClaimAndPrepare(ClaimToken {
                    fence: 6,
                    ..claim()
                }))
                .unwrap()
                .state,
            OperationState::CommitPrepared(ClaimToken {
                fence: 6,
                ..claim()
            })
        );
    }

    #[test]
    fn renew_requires_the_current_claim_owner_fence_and_revision() {
        let current = ClaimToken {
            owner: "runtime-a".into(),
            fence: 7,
            lease_until_ms: 100,
            heartbeat_revision: 2,
        };
        let operation = OperationRecord {
            id: id(),
            kind: OperationKind::ExecuteTask,
            state: OperationState::Claimed(current.clone()),
            revision: 1,
        };
        let renew = |current: ClaimToken, renewed: ClaimToken| {
            operation
                .clone()
                .evolve(OperationEvent::RenewClaim { current, renewed })
                .is_ok()
        };
        let renewed = ClaimToken {
            lease_until_ms: 200,
            heartbeat_revision: 3,
            ..current.clone()
        };
        assert!(renew(current.clone(), renewed));
        // A different expected claim.
        assert!(!renew(
            ClaimToken {
                fence: 8,
                ..current.clone()
            },
            ClaimToken {
                lease_until_ms: 200,
                heartbeat_revision: 3,
                ..current.clone()
            },
        ));
        // A different owner.
        assert!(!renew(
            current.clone(),
            ClaimToken {
                owner: "runtime-b".into(),
                lease_until_ms: 200,
                heartbeat_revision: 3,
                ..current.clone()
            },
        ));
        // A different fence.
        assert!(!renew(
            current.clone(),
            ClaimToken {
                fence: 8,
                lease_until_ms: 200,
                heartbeat_revision: 3,
                ..current.clone()
            },
        ));
        // A heartbeat that did not advance by exactly one.
        assert!(!renew(
            current.clone(),
            ClaimToken {
                lease_until_ms: 200,
                heartbeat_revision: 4,
                ..current.clone()
            },
        ));
    }

    #[test]
    fn commit_and_complete_require_the_exact_token() {
        let claim = claim();
        let commit = commit();
        let prepared = OperationRecord {
            id: id(),
            kind: OperationKind::ExecuteTask,
            state: OperationState::CommitPrepared(claim.clone()),
            revision: 1,
        };
        assert!(
            prepared
                .clone()
                .evolve(OperationEvent::Commit {
                    claim: ClaimToken {
                        fence: 8,
                        ..claim.clone()
                    },
                    commit: commit.clone(),
                })
                .is_err()
        );
        let committed = prepared
            .evolve(OperationEvent::Commit {
                claim,
                commit: commit.clone(),
            })
            .unwrap();
        assert!(
            committed
                .clone()
                .evolve(OperationEvent::Complete {
                    commit: CommitToken {
                        process_version: 12,
                        digest: "other".into(),
                    },
                })
                .is_err()
        );
        assert!(matches!(
            committed
                .evolve(OperationEvent::Complete { commit })
                .unwrap()
                .state,
            OperationState::Completed(_)
        ));
    }

    #[test]
    fn retry_and_unknown_attempts_must_match_the_fence_and_be_positive() {
        let current = ClaimToken {
            owner: "runtime-a".into(),
            fence: 7,
            lease_until_ms: 100,
            heartbeat_revision: 0,
        };
        let claimed = OperationRecord {
            id: id(),
            kind: OperationKind::ApplyAction,
            state: OperationState::Claimed(current.clone()),
            revision: 1,
        };
        for attempt in [0, 6, 7] {
            // 7 is the fence (accepted); 0 and 6 are rejected.
            let retry = claimed.clone().evolve(OperationEvent::ScheduleRetry {
                claim: current.clone(),
                attempt,
            });
            assert_eq!(retry.is_ok(), attempt == 7, "retry attempt {attempt}");
            let unknown = claimed.clone().evolve(OperationEvent::MarkEffectUnknown {
                claim: current.clone(),
                attempt,
            });
            assert_eq!(unknown.is_ok(), attempt == 7, "unknown attempt {attempt}");
        }
        // Resolving a proven-not-applied effect requires a positive attempt
        // that matches the persisted one.
        let unknown = OperationRecord {
            id: id(),
            kind: OperationKind::ApplyAction,
            state: OperationState::EffectUnknown { attempt: 4 },
            revision: 1,
        };
        assert!(
            unknown
                .clone()
                .evolve(OperationEvent::ResolveEffectNotApplied { attempt: 0 })
                .is_err()
        );
        assert!(
            unknown
                .evolve(OperationEvent::ResolveEffectNotApplied { attempt: 4 })
                .is_ok()
        );
    }

    #[test]
    fn release_expired_claim_requires_an_expired_claim_with_a_storable_fence() {
        let prepared = OperationRecord {
            id: id(),
            kind: OperationKind::ExecuteTask,
            state: OperationState::CommitPrepared(ClaimToken {
                lease_until_ms: 100,
                ..claim()
            }),
            revision: 1,
        };
        assert!(
            prepared
                .clone()
                .evolve(OperationEvent::ReleaseExpiredClaim { now_ms: 99 })
                .is_err(),
            "a live prepared claim cannot be released"
        );
        // A fence above u32::MAX cannot be reported as an attempt, so the
        // guard refuses instead of truncating.
        let overflowing = OperationRecord {
            id: id(),
            kind: OperationKind::ExecuteTask,
            state: OperationState::Claimed(ClaimToken {
                fence: u64::from(u32::MAX) + 1,
                lease_until_ms: 100,
                ..claim()
            }),
            revision: 1,
        };
        assert!(
            overflowing
                .evolve(OperationEvent::ReleaseExpiredClaim { now_ms: 200 })
                .is_err()
        );
    }

    #[test]
    fn quarantine_needs_a_reason_and_release_needs_a_positive_attempt() {
        let current = claim();
        let claimed = OperationRecord {
            id: id(),
            kind: OperationKind::ExecuteTask,
            state: OperationState::Claimed(current.clone()),
            revision: 1,
        };
        assert!(
            claimed
                .clone()
                .evolve(OperationEvent::Quarantine {
                    claim: current.clone(),
                    reason: " ".into(),
                })
                .is_err(),
            "an empty reason is not a quarantine"
        );
        let quarantined = claimed
            .evolve(OperationEvent::Quarantine {
                claim: current,
                reason: "hold".into(),
            })
            .unwrap();
        assert!(
            quarantined
                .clone()
                .evolve(OperationEvent::ReleaseQuarantine { next_attempt: 0 })
                .is_err()
        );
        assert!(
            quarantined
                .evolve(OperationEvent::ReleaseQuarantine { next_attempt: 1 })
                .is_ok()
        );
    }
}

#[cfg(test)]
mod state_alternation_tests {
    use super::*;

    #[test]
    fn an_unknown_effect_is_claimed_by_a_higher_fence() {
        let id = OperationId {
            process_id: "p".into(),
            task: TaskId::new("t", 2).unwrap(),
            sequence: 1,
        };
        let unknown = OperationRecord {
            id,
            kind: OperationKind::ApplyAction,
            state: OperationState::EffectUnknown { attempt: 3 },
            revision: 1,
        };
        let claim = ClaimToken {
            owner: "w".into(),
            fence: 4,
            lease_until_ms: 100,
            heartbeat_revision: 0,
        };
        assert_eq!(
            unknown
                .evolve(OperationEvent::Claim(claim.clone()))
                .unwrap()
                .state,
            OperationState::Claimed(claim)
        );
    }

    fn record(state: OperationState) -> OperationRecord {
        OperationRecord {
            id: OperationId {
                process_id: "p".into(),
                task: TaskId::new("t", 2).unwrap(),
                sequence: 1,
            },
            kind: OperationKind::ExecuteTask,
            state,
            revision: 1,
        }
    }

    fn token(owner: &str, fence: u64, lease_until_ms: i64) -> ClaimToken {
        ClaimToken {
            owner: owner.into(),
            fence,
            lease_until_ms,
            heartbeat_revision: 0,
        }
    }

    /// Both unstarted-but-retried states require a fresh, valid claim whose
    /// fence advances past the attempt they carry.
    #[test]
    fn retried_and_unknown_claims_require_a_fresh_valid_fence() {
        for state in [
            OperationState::RetryScheduled { attempt: 3 },
            OperationState::EffectUnknown { attempt: 3 },
        ] {
            let operation = record(state.clone());
            for invalid in [
                token(" ", 4, i64::MAX),
                token("w", 0, i64::MAX),
                token("w", 3, i64::MAX),
                token("w", 2, i64::MAX),
                token("w", 4, i64::MIN),
            ] {
                assert!(
                    operation
                        .clone()
                        .evolve(OperationEvent::Claim(invalid.clone()))
                        .is_err(),
                    "{state:?} must refuse {invalid:?}"
                );
                assert!(
                    operation
                        .clone()
                        .evolve(OperationEvent::ClaimAndPrepare(invalid.clone()))
                        .is_err(),
                    "{state:?} must refuse to prepare {invalid:?}"
                );
            }
            let next = token("w", 4, i64::MAX);
            assert_eq!(
                operation
                    .clone()
                    .evolve(OperationEvent::Claim(next.clone()))
                    .unwrap()
                    .state,
                OperationState::Claimed(next.clone())
            );
            assert_eq!(
                record(state)
                    .evolve(OperationEvent::ClaimAndPrepare(next.clone()))
                    .unwrap()
                    .state,
                OperationState::CommitPrepared(next)
            );
        }
    }

    /// A prepared operation can cross the effect boundary or be proven not to
    /// have crossed it, but only at its own attempt.
    #[test]
    fn prepared_unknown_and_resolve_guards_reject_the_wrong_attempt() {
        let prepared = record(OperationState::CommitPrepared(token("w", 5, 100)));
        for attempt in [0, 4, 6] {
            assert!(
                prepared
                    .clone()
                    .evolve(OperationEvent::MarkEffectUnknown {
                        claim: token("w", 5, 100),
                        attempt,
                    })
                    .is_err(),
                "attempt {attempt} is not the prepared fence"
            );
        }
        assert_eq!(
            prepared
                .evolve(OperationEvent::MarkEffectUnknown {
                    claim: token("w", 5, 100),
                    attempt: 5,
                })
                .unwrap()
                .state,
            OperationState::EffectUnknown { attempt: 5 }
        );

        let unknown = record(OperationState::EffectUnknown { attempt: 5 });
        for attempt in [0, 4, 6] {
            assert!(
                unknown
                    .clone()
                    .evolve(OperationEvent::ResolveEffectNotApplied { attempt })
                    .is_err(),
                "attempt {attempt} is not the unknown attempt"
            );
        }
        assert!(
            record(OperationState::RetryScheduled { attempt: 5 })
                .evolve(OperationEvent::ResolveEffectNotApplied { attempt: 5 })
                .is_err(),
            "only an EffectUnknown operation may be resolved"
        );
        assert_eq!(
            unknown
                .evolve(OperationEvent::ResolveEffectNotApplied { attempt: 5 })
                .unwrap()
                .state,
            OperationState::RetryScheduled { attempt: 5 }
        );
    }

    /// A prepared claim released by its expiry is reconciliation work, and its
    /// fence must fit the attempt the unknown state stores.
    #[test]
    fn prepared_release_expiry_requires_an_expired_storable_fence() {
        let live = record(OperationState::CommitPrepared(token("w", 5, 100)));
        assert!(
            live.evolve(OperationEvent::ReleaseExpiredClaim { now_ms: 99 })
                .is_err()
        );
        let overflowing = record(OperationState::CommitPrepared(token(
            "w",
            u64::from(u32::MAX) + 1,
            100,
        )));
        assert!(
            overflowing
                .evolve(OperationEvent::ReleaseExpiredClaim { now_ms: 200 })
                .is_err()
        );
        assert_eq!(
            record(OperationState::CommitPrepared(token(
                "w",
                u64::from(u32::MAX),
                100
            )))
            .evolve(OperationEvent::ReleaseExpiredClaim { now_ms: 200 })
            .unwrap()
            .state,
            OperationState::EffectUnknown { attempt: u32::MAX }
        );
    }
}
