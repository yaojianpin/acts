//! Compatibility-safe references to an execution generation.
//!
//! Generation zero in legacy rows means "use the resident generation" rather
//! than a real generation. Keeping that distinction explicit prevents wildcard
//! comparisons from leaking through recovery code.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GenerationReference {
    LegacyResident,
    Exact(i64),
}

impl GenerationReference {
    pub(crate) const fn from_persisted(value: i64) -> Self {
        if value == 0 {
            Self::LegacyResident
        } else {
            Self::Exact(value)
        }
    }

    pub(crate) const fn persisted(self) -> i64 {
        match self {
            Self::LegacyResident => 0,
            Self::Exact(generation) => generation,
        }
    }

    pub(crate) const fn resolve(self, resident_generation: i64) -> i64 {
        match self {
            Self::LegacyResident => resident_generation,
            Self::Exact(generation) => generation,
        }
    }

    pub(crate) const fn exactly_matches(self, resident_generation: i64) -> bool {
        matches!(self, Self::Exact(generation) if generation == resident_generation)
    }

    /// Compatibility selection for pre-generation rows. This may locate a
    /// legacy candidate but must never be used as proof of exclusive ownership.
    pub(crate) const fn compatibly_selects(self, resident_generation: i64) -> bool {
        match self {
            Self::LegacyResident => true,
            Self::Exact(generation) => generation == resident_generation,
        }
    }

    pub(crate) const fn conflicts_with(self, resident_generation: i64) -> bool {
        matches!(self, Self::Exact(generation) if generation != resident_generation)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct HandoffIdentity<'a> {
    pub(crate) generation: i64,
    pub(crate) target_task_id: Option<&'a str>,
}

impl GenerationReference {
    pub(crate) fn describes_handoff(
        self,
        expected_target: Option<&str>,
        resident: HandoffIdentity<'_>,
    ) -> bool {
        self.exactly_matches(resident.generation) && expected_target == resident.target_task_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_reference_resolves_but_never_proves_exact_ownership() {
        let legacy = GenerationReference::from_persisted(0);
        assert_eq!(legacy.resolve(7), 7);
        assert!(!legacy.exactly_matches(7));
    }

    #[test]
    fn handoff_requires_exact_generation_and_target() {
        let reference = GenerationReference::from_persisted(42);
        assert!(reference.describes_handoff(
            Some("parent"),
            HandoffIdentity {
                generation: 42,
                target_task_id: Some("parent")
            }
        ));
        assert!(!reference.describes_handoff(
            Some("parent"),
            HandoffIdentity {
                generation: 41,
                target_task_id: Some("parent")
            }
        ));
    }
}

/// Generation and handoff identity decoded from one durable recovery record.
/// Keeping both values together prevents adapters from resolving a generation
/// with one compatibility rule and validating its handoff with another.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecoveryGeneration {
    reference: GenerationReference,
    target_task_id: Option<String>,
}

impl RecoveryGeneration {
    pub(crate) fn from_persisted(source_generation: i64, target_task_id: Option<String>) -> Self {
        Self {
            reference: GenerationReference::from_persisted(source_generation),
            target_task_id,
        }
    }

    pub(crate) const fn persisted(&self) -> i64 {
        self.reference.persisted()
    }

    pub(crate) const fn resolve(&self, resident_generation: i64) -> i64 {
        self.reference.resolve(resident_generation)
    }

    pub(crate) fn describes_handoff(
        &self,
        resident_generation: i64,
        resident_parent_task_id: Option<&str>,
    ) -> bool {
        self.reference.describes_handoff(
            self.target_task_id.as_deref(),
            HandoffIdentity {
                generation: resident_generation,
                target_task_id: resident_parent_task_id,
            },
        )
    }
}
