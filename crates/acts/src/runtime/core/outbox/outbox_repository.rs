use crate::runtime::core::outbox::records::{OutboxError, OutboxId, OutboxRecord};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OutboxCursor(pub(crate) OutboxId);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OutboxPage {
    pub(crate) records: Vec<OutboxRecord>,
    pub(crate) next: Option<OutboxCursor>,
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
    CorruptData(String),
    ConcurrentModification(OutboxId),
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
            Self::CorruptData(reason) => write!(f, "corrupt outbox data: {reason}"),
            Self::ConcurrentModification(id) => {
                write!(f, "outbox record {} modified concurrently", id.key)
            }
            Self::Unavailable(reason) => write!(f, "outbox repository unavailable: {reason}"),
        }
    }
}
impl std::error::Error for OutboxRepositoryError {}
