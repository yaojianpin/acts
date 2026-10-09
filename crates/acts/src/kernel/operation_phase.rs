//! Durable operation effect progress, independent of persistence encoding.

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum OperationPhase {
    Created,
    DurablePending,
    Dispatched,
    EffectInFlight,
    EffectDurable,
    Completed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PhaseTransition {
    Advance,
    Idempotent,
    Reject,
}

impl OperationPhase {
    const fn rank(self) -> u8 {
        match self {
            Self::Created => 0,
            Self::DurablePending => 1,
            Self::Dispatched => 2,
            Self::EffectInFlight => 3,
            Self::EffectDurable => 4,
            Self::Completed => 5,
        }
    }

    pub(crate) fn transition_to(self, next: Self) -> PhaseTransition {
        if self == next {
            return PhaseTransition::Idempotent;
        }
        if next == Self::Completed {
            return if self == Self::EffectDurable {
                PhaseTransition::Advance
            } else {
                PhaseTransition::Reject
            };
        }
        if next.rank() > self.rank() {
            PhaseTransition::Advance
        } else {
            PhaseTransition::Reject
        }
    }

    pub(crate) const fn effect_is_durable(self) -> bool {
        matches!(self, Self::EffectDurable | Self::Completed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transitions_distinguish_advance_idempotence_and_rejection() {
        assert_eq!(
            OperationPhase::DurablePending.transition_to(OperationPhase::EffectInFlight),
            PhaseTransition::Advance
        );
        assert_eq!(
            OperationPhase::EffectInFlight.transition_to(OperationPhase::EffectInFlight),
            PhaseTransition::Idempotent
        );
        assert_eq!(
            OperationPhase::EffectDurable.transition_to(OperationPhase::Completed),
            PhaseTransition::Advance
        );
        assert_eq!(
            OperationPhase::DurablePending.transition_to(OperationPhase::Completed),
            PhaseTransition::Reject
        );
        assert_eq!(
            OperationPhase::Completed.transition_to(OperationPhase::EffectInFlight),
            PhaseTransition::Reject
        );
    }
}
