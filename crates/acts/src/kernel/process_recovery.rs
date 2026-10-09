//! Pure process-level recovery classification.
//!
//! Storage adapters translate their persisted state representation into
//! `DurableProcessState`; runtime orchestration consumes only the resulting
//! disposition. This keeps restart semantics out of cache/capacity code.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DurableProcessState {
    Parked,
    Ready,
    Running,
    Pending,
    Interrupt,
    Completed,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProcessRecoveryDisposition {
    StartParked,
    ResumeInterrupted,
    Ignore,
}

pub(crate) struct ProcessRecoveryPolicy;

impl ProcessRecoveryPolicy {
    pub(crate) const fn classify(state: DurableProcessState) -> ProcessRecoveryDisposition {
        match state {
            DurableProcessState::Parked => ProcessRecoveryDisposition::StartParked,
            DurableProcessState::Ready
            | DurableProcessState::Running
            | DurableProcessState::Pending => ProcessRecoveryDisposition::ResumeInterrupted,
            DurableProcessState::Interrupt
            | DurableProcessState::Completed
            | DurableProcessState::Failed
            | DurableProcessState::Unknown => ProcessRecoveryDisposition::Ignore,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_recovery_classification_is_exhaustive() {
        assert_eq!(
            ProcessRecoveryPolicy::classify(DurableProcessState::Parked),
            ProcessRecoveryDisposition::StartParked
        );
        for state in [
            DurableProcessState::Ready,
            DurableProcessState::Running,
            DurableProcessState::Pending,
        ] {
            assert_eq!(
                ProcessRecoveryPolicy::classify(state),
                ProcessRecoveryDisposition::ResumeInterrupted
            );
        }
        for state in [
            DurableProcessState::Interrupt,
            DurableProcessState::Completed,
            DurableProcessState::Failed,
            DurableProcessState::Unknown,
        ] {
            assert_eq!(
                ProcessRecoveryPolicy::classify(state),
                ProcessRecoveryDisposition::Ignore
            );
        }
    }
}
