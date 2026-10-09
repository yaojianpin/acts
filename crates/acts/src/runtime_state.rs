//! Runtime-side identity of one durable operation.
//!
//! The kernel owns every interpretation of a recovery fact; this module owns
//! the identity handle the runtime matches against resident objects. It lives
//! outside `kernel/` because the architecture rules forbid the kernel from
//! depending on runtime vocabulary, while `kernel::RecoveryCandidate` still
//! needs a typed identity to hand the recovery orchestrator.

use crate::kernel::ReplayOperationKind;

/// Stable identity of one durable operation row, decoded beside its recovery
/// fact. `source_version` is the generation the row was written for; a
/// resident task whose generation no longer matches must not replay it (see
/// `kernel::RecoveryGeneration::describes_handoff` for the kernel-side rule).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OperationIdentity {
    pid: String,
    tid: String,
    kind: ReplayOperationKind,
    source_version: i64,
}

impl OperationIdentity {
    pub(crate) fn new(
        pid: &str,
        tid: &str,
        kind: ReplayOperationKind,
        source_version: i64,
    ) -> Self {
        Self {
            pid: pid.to_string(),
            tid: tid.to_string(),
            kind,
            source_version,
        }
    }

    pub(crate) fn pid(&self) -> &str {
        &self.pid
    }

    pub(crate) fn tid(&self) -> &str {
        &self.tid
    }

    pub(crate) const fn kind(&self) -> ReplayOperationKind {
        self.kind
    }

    /// The generation this operation was written for; `0` is the legacy
    /// wildcard (`kernel::GenerationReference::LegacyResident`).
    pub(crate) const fn source_version(&self) -> i64 {
        self.source_version
    }
}
