//! Pure execution-generation admission policy.
//!
//! Resident objects, queues, caches and storage adapt into this policy. The
//! kernel owns the rule; scheduler code owns only atomic observation/mutation.

use crate::scheduler::core::plan::ActPlan;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TaskSlotKind {
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

/// The plan of one execution-generation admission: whether the execution a
/// node wants may take its slot, reuse one a rewind recreated, or is ignored.
pub(crate) struct Admit;

impl ActPlan for Admit {
    type Reading<'a> = ();
    type Command<'a> = (TaskSlotKind, AdmissionIntent, SlotObservation);
    type Output = AdmissionDecision;
    type Error = std::convert::Infallible;

    /// Decide one admission: the whole input is the command.
    fn plan(_reading: (), command: &Self::Command<'_>) -> Result<Self::Output, Self::Error> {
        let (kind, intent, observation) = *command;
        if observation.process_completed {
            return Ok(AdmissionDecision::Ignore);
        }
        if matches!(intent, AdmissionIntent::Forward)
            && matches!(kind, TaskSlotKind::Once)
            && observation.predecessor_failed
        {
            return Ok(AdmissionDecision::Ignore);
        }
        Ok(match (kind, observation.existing) {
            (_, ExistingGeneration::Unstarted) => AdmissionDecision::ReadmitExisting,
            (TaskSlotKind::Once, ExistingGeneration::Absent | ExistingGeneration::Completed) => {
                AdmissionDecision::AllocateGeneration
            }
            (TaskSlotKind::Once, ExistingGeneration::Active) => AdmissionDecision::Ignore,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::core::plan::infallible;

    #[test]
    fn terminal_process_and_failed_forward_are_rejected() {
        assert_eq!(
            infallible(Admit::plan(
                (),
                &(
                    TaskSlotKind::Once,
                    AdmissionIntent::Forward,
                    SlotObservation::vacant(true, false)
                )
            )),
            AdmissionDecision::Ignore
        );
        let failed = SlotObservation::vacant(false, true);
        assert_eq!(
            infallible(Admit::plan(
                (),
                &(TaskSlotKind::Once, AdmissionIntent::Forward, failed)
            )),
            AdmissionDecision::Ignore
        );
    }
}
