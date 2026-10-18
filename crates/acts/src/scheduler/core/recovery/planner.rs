use crate::scheduler::core::{
    commit::application::WakeError,
    model::operation::{OperationId, OperationRecord, OperationState},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RecoveryDisposition {
    /// A normal operation without a ready record: the repair path. This is
    /// unreachable for newly written aggregates (the ready index is written in
    /// the same batch), but an old database or a damaged index converges here.
    RepairReady,
    AwaitOwner,
    ReclaimExpired,
    FinalizeCommit,
    ManualReconciliation,
    IgnoreTerminal,
}

pub(crate) struct RecoveryPlanner;

impl RecoveryPlanner {
    pub(crate) fn classify(operation: &OperationRecord, now_ms: i64) -> RecoveryDisposition {
        match &operation.state {
            OperationState::Pending | OperationState::RetryScheduled { .. } => {
                RecoveryDisposition::RepairReady
            }
            OperationState::Claimed(claim) | OperationState::CommitPrepared(claim) => {
                if claim.is_expired(now_ms) {
                    RecoveryDisposition::ReclaimExpired
                } else {
                    RecoveryDisposition::AwaitOwner
                }
            }
            OperationState::Committed(_) => RecoveryDisposition::FinalizeCommit,
            OperationState::EffectUnknown { .. } | OperationState::Quarantined { .. } => {
                RecoveryDisposition::ManualReconciliation
            }
            OperationState::Superseded { .. } | OperationState::Completed(_) => {
                RecoveryDisposition::IgnoreTerminal
            }
        }
    }
}

/// What one recovery scan did.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct RecoveryReport {
    pub(crate) scanned: usize,
    /// Operations whose expired claim was reclaimed.
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
