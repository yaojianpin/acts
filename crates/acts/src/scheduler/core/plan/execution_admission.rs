//! Pure execution-generation admission policy.
//!
//! Resident objects, queues, caches and storage adapt into this policy. The
//! kernel owns the rule; scheduler code owns only atomic observation/mutation.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ExecutionSlotKind {
    Once,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdmissionIntent {
    Forward,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SlotObservation {
    process_completed: bool,
    predecessor_failed: bool,
    existing: ExistingGeneration,
}
impl SlotObservation {
    pub(crate) const fn vacant(process_completed: bool, predecessor_failed: bool) -> Self {
        Self {
            process_completed,
            predecessor_failed,
            existing: ExistingGeneration::Absent,
        }
    }
    pub(crate) const fn with_existing(self, existing: ExistingGeneration) -> Self {
        Self { existing, ..self }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExistingGeneration {
    Absent,
    Unstarted,
    Active,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdmissionDecision {
    Ignore,
    ReadmitExisting,
    AllocateGeneration,
}

pub(crate) struct GenerationAdmissionPolicy;
impl GenerationAdmissionPolicy {
    pub(crate) const fn decide(
        kind: ExecutionSlotKind,
        intent: AdmissionIntent,
        observation: SlotObservation,
    ) -> AdmissionDecision {
        if observation.process_completed {
            return AdmissionDecision::Ignore;
        }
        if matches!(intent, AdmissionIntent::Forward)
            && matches!(kind, ExecutionSlotKind::Once)
            && observation.predecessor_failed
        {
            return AdmissionDecision::Ignore;
        }
        match (kind, observation.existing) {
            (_, ExistingGeneration::Unstarted) => AdmissionDecision::ReadmitExisting,
            (
                ExecutionSlotKind::Once,
                ExistingGeneration::Absent | ExistingGeneration::Completed,
            ) => AdmissionDecision::AllocateGeneration,
            (ExecutionSlotKind::Once, ExistingGeneration::Active) => AdmissionDecision::Ignore,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_process_and_failed_forward_are_rejected() {
        assert_eq!(
            GenerationAdmissionPolicy::decide(
                ExecutionSlotKind::Once,
                AdmissionIntent::Forward,
                SlotObservation::vacant(true, false)
            ),
            AdmissionDecision::Ignore
        );
        let failed = SlotObservation::vacant(false, true);
        assert_eq!(
            GenerationAdmissionPolicy::decide(
                ExecutionSlotKind::Once,
                AdmissionIntent::Forward,
                failed
            ),
            AdmissionDecision::Ignore
        );
    }
}
