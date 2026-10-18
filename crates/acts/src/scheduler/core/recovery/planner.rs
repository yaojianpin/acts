//! Recovery's single command: observe one operation, decide what it means.
//!
//! Recovery used to classify an operation in `core` and branch on the
//! classification in the shell: the shell decided when an expired claim was
//! woken, when a committed operation was finalized, and how an effect
//! observation was split between replay, retry and a human. That left two
//! implementations of the same lifecycle question — the normal operation
//! pipeline and `recover()` — free to drift (A7).
//!
//! This module is the one entry. [`Observed`] is the complete fact set recovery
//! may collect (`operation_id`, persisted phase with its claim/lease,
//! aggregate revision, effect receipt, now); [`recover_operation`] is a pure
//! function from that fact set to [`RecoveryDecision`]. The decision carries
//! the operation event the shell must commit under the pid's command lane, if
//! any, and a [`RecoveryDirective`] naming the IO the shell must perform.
//! Nothing here reads a store, takes a lock or calls an executor.
//!
//! The normal path and recovery converge on the same protocol:
//!
//! - a mutating recovery command is an [`OperationEvent`] committed with the
//!   same revision guard and the same per-pid lane as a normal operation;
//! - after the commit, the shell builds the same
//!   [`Decision`](crate::scheduler::core::decision::Decision) the normal
//!   `dispatch` builds and executes it with the same post-commit program
//!   (`project` → lifecycle → outbox confirmation → admission);
//! - an effect observation is still A1c's: only `Applied`/`Failed` may replay
//!   the plan, only a provably idempotent `NotApplied` may re-queue the
//!   operation, and everything else stays for a human.

use crate::scheduler::core::{
    commit::application::WakeError,
    model::{
        operation::{
            ClaimToken, OperationEvent, OperationId, OperationKind, OperationRecord, OperationState,
        },
        transition_plan::ProcessVersion,
    },
};

/// What recovery observed about one operation. This is the whole input to
/// [`recover_operation`]; every field is a durable fact or the clock.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Observed {
    pub(crate) operation_id: OperationId,
    pub(crate) kind: OperationKind,
    /// The authoritative phase, including the claim token (and therefore the
    /// lease) or the commit token the phase carries.
    pub(crate) persisted_phase: OperationState,
    /// The operation revision the command's guard is checked against.
    pub(crate) revision: u64,
    /// The aggregate revision the operation was read with. Recovery commits
    /// nothing against it (the operation event's own guard is the fence); it
    /// is the observation's version, kept so a decision can be replayed.
    pub(crate) aggregate_revision: ProcessVersion,
    /// The executor's own proof about the effect, when the phase is
    /// `EffectUnknown`. `Unobserved` means recovery has not asked yet.
    pub(crate) effect_receipt: EffectReceipt,
    pub(crate) now_ms: i64,
}

impl Observed {
    /// The fact set of one recovery-index row, before any effect is observed.
    pub(crate) fn from_record(
        operation: &OperationRecord,
        aggregate_revision: ProcessVersion,
        now_ms: i64,
    ) -> Self {
        Self {
            operation_id: operation.id.clone(),
            kind: operation.kind,
            persisted_phase: operation.state.clone(),
            revision: operation.revision,
            aggregate_revision,
            effect_receipt: EffectReceipt::Unobserved,
            now_ms,
        }
    }

    /// The same observation, with the executor's proof folded in.
    pub(crate) fn with_effect_receipt(self, effect_receipt: EffectReceipt) -> Self {
        Self {
            effect_receipt,
            ..self
        }
    }

    /// The claim/lease the persisted phase carries, if any.
    pub(crate) fn claim(&self) -> Option<&ClaimToken> {
        match &self.persisted_phase {
            OperationState::Claimed(claim) | OperationState::CommitPrepared(claim) => Some(claim),
            _ => None,
        }
    }

    /// Whether a claimed phase's lease has expired at the observed instant.
    pub(crate) fn lease_expired(&self) -> bool {
        self.claim()
            .is_some_and(|claim| claim.is_expired(self.now_ms))
    }

    /// The record this observation describes. Recovery's effect paths take the
    /// same `OperationRecord` API the operator path uses, so the state and the
    /// revision stay the ones the decision was formed from.
    pub(crate) fn record(&self) -> OperationRecord {
        OperationRecord {
            id: self.operation_id.clone(),
            kind: self.kind,
            state: self.persisted_phase.clone(),
            revision: self.revision,
        }
    }
}

/// What one executor can prove about one effect (A1c), reduced to the facts a
/// decision needs. The payload (outputs, failure text) stays with the shell:
/// it is the executor's vocabulary, and only a `ReplayEffect` directive makes
/// the shell hand it back to the normal planner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EffectReceipt {
    /// Not asked: the phase is not `EffectUnknown`, or recovery has not taken
    /// the observation yet.
    Unobserved,
    /// The effect ran.
    Applied,
    /// The effect ran and failed.
    Failed,
    /// The effect did not run; `idempotent` is the package's own declaration
    /// that the same effect key may be executed again.
    NotApplied { idempotent: bool },
    /// The executor cannot prove either way.
    Unknown,
}

/// The operation lifecycle event a recovery command commits, with the guard
/// the normal path uses: the same per-pid lane outside, the same operation
/// revision fence inside.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecoveryCommand {
    pub(crate) event: OperationEvent,
    pub(crate) expected_revision: u64,
}

/// What the shell must do for one observed operation. Every business question
/// (is this claim expired, is this effect replayable, is this retry safe) is
/// already answered; the variants name IO.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RecoveryDirective {
    /// A live owner holds the lease: leave it alone.
    AwaitOwner,
    /// The phase is terminal history: nothing to do.
    IgnoreTerminal,
    /// Normal work whose durable ready record is missing (an older database,
    /// a damaged index): rebuild it.
    RepairReady,
    /// An expired claim on a phase that has not crossed the effect boundary:
    /// commit `ReleaseExpiredClaim` (which writes the ready record in the same
    /// batch), then wake it.
    ReclaimReady,
    /// An expired claim on `CommitPrepared`: the executor may already have
    /// run the effect. Commit `ReleaseExpiredClaim` (which turns the
    /// operation `EffectUnknown`), then observe it — never re-drive it.
    ReclaimUnknown,
    /// A commit whose completion was lost (an older writer's state): close it
    /// with `Complete`, then run the same post-commit program a normal
    /// `dispatch` runs.
    FinalizeCommit,
    /// Ask the executor for its effect observation, then decide again.
    ObserveEffect,
    /// The executor proved the effect ran: replay the plan without running
    /// the effect.
    ReplayEffect,
    /// The executor proved an idempotent effect did not run: re-queue it under
    /// the same identity.
    RetryEffect,
    /// Nothing can be proven (or the phase is quarantined): a human owns it.
    Manual,
}

/// The decision for one observation: the business directive plus the command
/// it requires, if any.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecoveryDecision {
    pub(crate) directive: RecoveryDirective,
    pub(crate) command: Option<RecoveryCommand>,
}

/// Recovery's only entry: a pure decision from the observed facts. This is the
/// recovery twin of the normal pipeline's plan-and-dispatch decision; it is
/// deliberately not a second state machine.
pub(crate) fn recover_operation(observed: &Observed) -> RecoveryDecision {
    let directive = RecoveryPlanner::classify(observed);
    let command = match (&directive, &observed.persisted_phase) {
        (
            RecoveryDirective::ReclaimReady | RecoveryDirective::ReclaimUnknown,
            OperationState::Claimed(_) | OperationState::CommitPrepared(_),
        ) => Some(RecoveryCommand {
            event: OperationEvent::ReleaseExpiredClaim {
                now_ms: observed.now_ms,
            },
            expected_revision: observed.revision,
        }),
        (RecoveryDirective::FinalizeCommit, OperationState::Committed(commit)) => {
            Some(RecoveryCommand {
                event: OperationEvent::Complete {
                    commit: commit.clone(),
                },
                expected_revision: observed.revision,
            })
        }
        _ => None,
    };
    RecoveryDecision { directive, command }
}

/// The classification is an implementation detail of [`recover_operation`]:
/// the shell never matches on a phase.
struct RecoveryPlanner;

impl RecoveryPlanner {
    fn classify(observed: &Observed) -> RecoveryDirective {
        use OperationState as S;
        match &observed.persisted_phase {
            S::Pending | S::RetryScheduled { .. } => RecoveryDirective::RepairReady,
            S::Claimed(_) | S::CommitPrepared(_) => {
                if !observed.lease_expired() {
                    RecoveryDirective::AwaitOwner
                } else if matches!(observed.persisted_phase, S::Claimed(_)) {
                    // The handler had not crossed the effect boundary: the
                    // claim is simply released for the ready queue.
                    RecoveryDirective::ReclaimReady
                } else {
                    // `CommitPrepared` is the effect boundary: a crash may
                    // have happened after the effect ran, so the reclaimed
                    // state is `EffectUnknown` and only an observation may
                    // move it.
                    RecoveryDirective::ReclaimUnknown
                }
            }
            S::Committed(_) => RecoveryDirective::FinalizeCommit,
            S::EffectUnknown { .. } => classify_effect(observed),
            S::Quarantined { .. } => RecoveryDirective::Manual,
            S::Superseded { .. } | S::Completed(_) => RecoveryDirective::IgnoreTerminal,
        }
    }
}

/// A1c's split, in one place: only an executor proof moves an unknown effect;
/// a proof that it ran replays the plan, a proof that it did not run retries
/// only a declared-idempotent effect, and everything else stays manual.
fn classify_effect(observed: &Observed) -> RecoveryDirective {
    if observed.kind != OperationKind::ApplyAction {
        return RecoveryDirective::Manual;
    }
    match observed.effect_receipt {
        EffectReceipt::Unobserved => RecoveryDirective::ObserveEffect,
        EffectReceipt::Applied | EffectReceipt::Failed => RecoveryDirective::ReplayEffect,
        EffectReceipt::NotApplied { idempotent: true } => RecoveryDirective::RetryEffect,
        EffectReceipt::NotApplied { idempotent: false } | EffectReceipt::Unknown => {
            RecoveryDirective::Manual
        }
    }
}

/// What one recovery scan did.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct RecoveryReport {
    pub(crate) scanned: usize,
    /// Operations whose expired claim was reclaimed into the ready queue.
    pub(crate) reclaimed: Vec<OperationId>,
    /// Normal operations whose durable ready record was missing and repaired.
    pub(crate) ready_repaired: Vec<OperationId>,
    pub(crate) awaiting_owner: Vec<OperationId>,
    pub(crate) finalized: Vec<OperationId>,
    /// `EffectUnknown` operations whose executor observation proved the
    /// effect ran; the plan was replayed without calling the effect again.
    pub(crate) effect_applied: Vec<OperationId>,
    /// `EffectUnknown` operations whose observation proved the effect did not
    /// run and whose effect is idempotent; they are back in the ready queue.
    pub(crate) effect_retried: Vec<OperationId>,
    pub(crate) reconciliation_required: Vec<OperationId>,
    pub(crate) wake_failures: Vec<(OperationId, WakeError)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::core::model::{operation::CommitToken, task_graph::TaskId};

    fn id() -> OperationId {
        OperationId {
            process_id: "p".into(),
            task: TaskId::new("step", 0).unwrap(),
            sequence: 0,
        }
    }

    fn claim(lease_until_ms: i64) -> ClaimToken {
        ClaimToken {
            owner: "owner".into(),
            fence: 1,
            lease_until_ms,
            heartbeat_revision: 0,
        }
    }

    fn commit() -> CommitToken {
        CommitToken {
            process_version: 2,
            digest: "d".into(),
        }
    }

    fn observed(phase: OperationState) -> Observed {
        Observed {
            operation_id: id(),
            kind: OperationKind::ApplyAction,
            persisted_phase: phase,
            revision: 7,
            aggregate_revision: ProcessVersion {
                graph: 1,
                process: 2,
                operations: 3,
            },
            effect_receipt: EffectReceipt::Unobserved,
            now_ms: 1_000,
        }
    }

    fn directive(phase: OperationState) -> RecoveryDirective {
        recover_operation(&observed(phase)).directive
    }

    #[test]
    fn normal_work_is_repaired_and_terminal_history_is_ignored() {
        assert_eq!(
            directive(OperationState::Pending),
            RecoveryDirective::RepairReady
        );
        assert_eq!(
            directive(OperationState::RetryScheduled { attempt: 2 }),
            RecoveryDirective::RepairReady
        );
        assert_eq!(
            directive(OperationState::Superseded { by_generation: 1 }),
            RecoveryDirective::IgnoreTerminal
        );
        assert_eq!(
            directive(OperationState::Completed(commit())),
            RecoveryDirective::IgnoreTerminal
        );
    }

    #[test]
    fn a_live_lease_waits_and_an_expired_one_is_reclaimed_under_the_lane() {
        let live = recover_operation(&observed(OperationState::Claimed(claim(1_001))));
        assert_eq!(live.directive, RecoveryDirective::AwaitOwner);
        assert_eq!(live.command, None);

        let claimed = observed(OperationState::Claimed(claim(1_000)));
        let decision = recover_operation(&claimed);
        assert_eq!(decision.directive, RecoveryDirective::ReclaimReady);
        assert_eq!(
            decision.command,
            Some(RecoveryCommand {
                event: OperationEvent::ReleaseExpiredClaim { now_ms: 1_000 },
                expected_revision: 7,
            })
        );

        let prepared = observed(OperationState::CommitPrepared(claim(1_000)));
        let decision = recover_operation(&prepared);
        assert_eq!(
            decision.directive,
            RecoveryDirective::ReclaimUnknown,
            "the effect boundary is never re-driven automatically"
        );
        assert!(matches!(
            decision.command.map(|command| command.event),
            Some(OperationEvent::ReleaseExpiredClaim { .. })
        ));

        assert_eq!(
            directive(OperationState::CommitPrepared(claim(1_001))),
            RecoveryDirective::AwaitOwner
        );
    }

    #[test]
    fn a_lost_completion_is_finalized_from_its_own_commit_token() {
        let decision = recover_operation(&observed(OperationState::Committed(commit())));
        assert_eq!(decision.directive, RecoveryDirective::FinalizeCommit);
        assert_eq!(
            decision.command,
            Some(RecoveryCommand {
                event: OperationEvent::Complete { commit: commit() },
                expected_revision: 7,
            })
        );
    }

    #[test]
    fn unknown_effects_follow_the_executor_proof_not_a_shell_branch() {
        let unknown = OperationState::EffectUnknown { attempt: 1 };
        assert_eq!(directive(unknown.clone()), RecoveryDirective::ObserveEffect);

        let receipt = |receipt| observed(unknown.clone()).with_effect_receipt(receipt);
        assert_eq!(
            recover_operation(&receipt(EffectReceipt::Applied)).directive,
            RecoveryDirective::ReplayEffect
        );
        assert_eq!(
            recover_operation(&receipt(EffectReceipt::Failed)).directive,
            RecoveryDirective::ReplayEffect
        );
        assert_eq!(
            recover_operation(&receipt(EffectReceipt::NotApplied { idempotent: true })).directive,
            RecoveryDirective::RetryEffect
        );
        assert_eq!(
            recover_operation(&receipt(EffectReceipt::NotApplied { idempotent: false })).directive,
            RecoveryDirective::Manual
        );
        assert_eq!(
            recover_operation(&receipt(EffectReceipt::Unknown)).directive,
            RecoveryDirective::Manual
        );
        assert_eq!(
            recover_operation(&receipt(EffectReceipt::Unobserved)).directive,
            RecoveryDirective::ObserveEffect
        );
    }

    #[test]
    fn a_non_effect_operation_in_an_unknown_state_stays_manual() {
        let mut observed = observed(OperationState::EffectUnknown { attempt: 1 });
        observed.kind = OperationKind::ExecuteTask;
        assert_eq!(
            recover_operation(&observed.with_effect_receipt(EffectReceipt::Applied)).directive,
            RecoveryDirective::Manual
        );
    }

    #[test]
    fn a_quarantined_operation_is_always_manual() {
        let mut observed = observed(OperationState::Quarantined {
            reason: "operator".into(),
        });
        assert_eq!(
            recover_operation(&observed).directive,
            RecoveryDirective::Manual
        );
        observed.effect_receipt = EffectReceipt::Applied;
        assert_eq!(
            recover_operation(&observed).directive,
            RecoveryDirective::Manual
        );
    }

    #[test]
    fn the_observation_carries_the_claim_and_the_lease() {
        let live = observed(OperationState::Claimed(claim(2_000)));
        assert_eq!(live.claim(), Some(&claim(2_000)));
        assert!(!live.lease_expired());

        let expired = observed(OperationState::Claimed(claim(1_000)));
        assert!(expired.lease_expired(), "now == lease_until is expired");

        let unclaimed = observed(OperationState::Pending);
        assert_eq!(unclaimed.claim(), None);
        assert!(!unclaimed.lease_expired());
    }
}
