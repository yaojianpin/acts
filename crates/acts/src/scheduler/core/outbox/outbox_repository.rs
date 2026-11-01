use crate::scheduler::core::outbox::records::{OutboxError, OutboxEvent, OutboxId, OutboxRecord};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OutboxCursor(pub(crate) OutboxId);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OutboxPage {
    pub(crate) records: Vec<OutboxRecord>,
    pub(crate) next: Option<OutboxCursor>,
    /// Index entries the page walked (due or not), for the retry loop's
    /// backlog gauge.
    pub(crate) observed: usize,
    /// Entries due at the scan instant (pending at or before `now`, or a
    /// claim whose lease expired).
    pub(crate) due: usize,
    /// The earliest due instant among this page's due entries, so the loop can
    /// report how far behind the oldest one is.
    pub(crate) oldest_due_at_ms: Option<i64>,
    /// The pids the page tried to load. A pid that does not appear in
    /// [`Self::failures`] was read cleanly, which is what clears an earlier
    /// poisoned entry for it.
    pub(crate) attempted: Vec<String>,
    /// Per-pid damage the page isolated. The page's other records are still
    /// returned: one bad run must not stop delivery for every other run. The
    /// loop records these pids, keeps its health degraded, retries them on the
    /// next pass, and asks the index repair to rewrite the derived documents.
    pub(crate) failures: Vec<OutboxPageFailure>,
}

/// One process whose outbox index or aggregate could not be used by a page.
/// The error text is the operator-facing record of what is wrong; the pid is
/// retried every pass until it reads cleanly or a repair rewrites its index.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OutboxPageFailure {
    pub(crate) process_id: String,
    pub(crate) error: String,
    /// Whether the derived-index repair can rewrite this run's outbox index
    /// from its aggregate (an index document that does not match its records)
    /// as opposed to a family that is itself inconsistent (no repair can
    /// invent the missing truth).
    pub(crate) repairable: bool,
}

/// One record's post-delivery facts, applied in memory before the one guarded
/// commit that persists the batch. The events are the existing state machine
/// (`claim` then `confirm`/`reschedule`/`unknown`, or `quarantine`): the claim
/// is only an in-memory fence so the persisted record keeps the same counters
/// a durable claim+confirm pair would leave.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OutboxConfirmation {
    pub(crate) id: OutboxId,
    pub(crate) expected_revision: u64,
    pub(crate) events: Vec<OutboxEvent>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OutboxBatchOutcome {
    /// The records the batch advanced. A record whose revision moved under
    /// the batch — a concurrent outbox pass claimed or settled it — is skipped
    /// and left to that pass.
    pub(crate) confirmed: Vec<OutboxRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OutboxRepositoryError {
    InvalidPageLimit,
    NotFound(OutboxId),
    RevisionConflict {
        id: OutboxId,
        expected: u64,
        actual: u64,
    },
    InvalidRecord(OutboxError),
    ConcurrentModification(OutboxId),
    ConcurrentProcess(String),
    Unavailable(String),
}

impl std::fmt::Display for OutboxRepositoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPageLimit => write!(f, "invalid outbox page limit"),
            Self::NotFound(id) => write!(f, "outbox record {} not found", id.key),
            Self::RevisionConflict {
                id,
                expected,
                actual,
            } => write!(
                f,
                "outbox record {} revision conflict: expected {expected}, found {actual}",
                id.key
            ),
            Self::InvalidRecord(error) => write!(f, "invalid outbox record: {error}"),
            Self::ConcurrentModification(id) => {
                write!(f, "outbox record {} modified concurrently", id.key)
            }
            Self::ConcurrentProcess(process_id) => {
                write!(f, "process {process_id} modified concurrently")
            }
            Self::Unavailable(reason) => write!(f, "outbox repository unavailable: {reason}"),
        }
    }
}
impl std::error::Error for OutboxRepositoryError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_repository_error_renders_its_context() {
        let id = OutboxId {
            process_id: "p".into(),
            key: "k".into(),
        };
        assert_eq!(
            OutboxRepositoryError::InvalidPageLimit.to_string(),
            "invalid outbox page limit"
        );
        assert_eq!(
            OutboxRepositoryError::NotFound(id.clone()).to_string(),
            "outbox record k not found"
        );
        assert_eq!(
            OutboxRepositoryError::RevisionConflict {
                id: id.clone(),
                expected: 2,
                actual: 3,
            }
            .to_string(),
            "outbox record k revision conflict: expected 2, found 3"
        );
        assert_eq!(
            OutboxRepositoryError::InvalidRecord(OutboxError::InvalidTopic).to_string(),
            "invalid outbox record: outbox error: InvalidTopic"
        );
        assert_eq!(
            OutboxRepositoryError::ConcurrentModification(id).to_string(),
            "outbox record k modified concurrently"
        );
        assert_eq!(
            OutboxRepositoryError::ConcurrentProcess("p".into()).to_string(),
            "process p modified concurrently"
        );
        assert_eq!(
            OutboxRepositoryError::Unavailable("offline".into()).to_string(),
            "outbox repository unavailable: offline"
        );
    }
}
