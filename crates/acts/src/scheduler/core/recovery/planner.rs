use crate::scheduler::core::{
    commit::application::WakeError,
    model::operation::{OperationId, OperationRecord, OperationState},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RecoveryDisposition {
    Wake,
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
                RecoveryDisposition::Wake
            }
            OperationState::Claimed(claim) | OperationState::CommitPrepared(claim) => {
                if claim.is_expired(now_ms) {
                    RecoveryDisposition::ReclaimExpired
                } else {
                    RecoveryDisposition::AwaitOwner
                }
            }
            OperationState::Committed(_) => RecoveryDisposition::FinalizeCommit,
            OperationState::EffectUnknown { .. } => RecoveryDisposition::ManualReconciliation,
            OperationState::Quarantined { .. }
            | OperationState::Superseded { .. }
            | OperationState::Completed(_) => RecoveryDisposition::IgnoreTerminal,
        }
    }
}

/// What one recovery scan did.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct RecoveryReport {
    pub(crate) scanned: usize,
    pub(crate) woken: Vec<OperationId>,
    pub(crate) wake_failures: Vec<(OperationId, WakeError)>,
    pub(crate) awaiting_owner: Vec<OperationId>,
    pub(crate) finalized: Vec<OperationId>,
    pub(crate) reconciliation_required: Vec<OperationId>,
}
