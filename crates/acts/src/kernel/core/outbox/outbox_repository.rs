use super::outbox::{DeliveryClaim, OutboxError, OutboxEvent, OutboxId, OutboxRecord};
use std::collections::HashMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OutboxCursor(pub(crate) OutboxId);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OutboxPage {
    pub(crate) records: Vec<OutboxRecord>,
    pub(crate) next: Option<OutboxCursor>,
}

pub(crate) trait OutboxRepository {
    fn load_outbox(&self, id: &OutboxId) -> Result<Option<OutboxRecord>, OutboxRepositoryError>;
    fn dispatchable_outbox_page(
        &self,
        now_ms: i64,
        after: Option<&OutboxCursor>,
        limit: usize,
    ) -> Result<OutboxPage, OutboxRepositoryError>;
    fn evolve_outbox(
        &mut self,
        id: &OutboxId,
        expected_revision: u64,
        event: OutboxEvent,
    ) -> Result<OutboxRecord, OutboxRepositoryError>;
}

#[derive(Default)]
pub(crate) struct MemoryOutboxRepository {
    records: HashMap<OutboxId, OutboxRecord>,
}

impl MemoryOutboxRepository {
    pub(crate) fn insert(&mut self, record: OutboxRecord) -> Result<(), OutboxRepositoryError> {
        record
            .validate()
            .map_err(OutboxRepositoryError::InvalidRecord)?;
        use std::collections::hash_map::Entry;
        match self.records.entry(record.id.clone()) {
            Entry::Vacant(slot) => {
                slot.insert(record);
                Ok(())
            }
            Entry::Occupied(_) => Err(OutboxRepositoryError::AlreadyExists(record.id)),
        }
    }
}

impl OutboxRepository for MemoryOutboxRepository {
    fn load_outbox(&self, id: &OutboxId) -> Result<Option<OutboxRecord>, OutboxRepositoryError> {
        Ok(self.records.get(id).cloned())
    }

    fn dispatchable_outbox_page(
        &self,
        now_ms: i64,
        after: Option<&OutboxCursor>,
        limit: usize,
    ) -> Result<OutboxPage, OutboxRepositoryError> {
        if limit == 0 {
            return Err(OutboxRepositoryError::InvalidPageLimit);
        }
        let mut records = self
            .records
            .values()
            .filter(|record| record.state.is_dispatchable(now_ms))
            .filter(|record| after.is_none_or(|cursor| record.id > cursor.0))
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by(|left, right| left.id.cmp(&right.id));
        let has_more = records.len() > limit;
        records.truncate(limit);
        let next = if has_more {
            records.last().map(|record| OutboxCursor(record.id.clone()))
        } else {
            None
        };
        Ok(OutboxPage { records, next })
    }

    fn evolve_outbox(
        &mut self,
        id: &OutboxId,
        expected_revision: u64,
        event: OutboxEvent,
    ) -> Result<OutboxRecord, OutboxRepositoryError> {
        let current = self
            .records
            .get(id)
            .ok_or_else(|| OutboxRepositoryError::NotFound(id.clone()))?;
        if current.revision != expected_revision {
            return Err(OutboxRepositoryError::RevisionConflict {
                id: id.clone(),
                expected: expected_revision,
                actual: current.revision,
            });
        }
        let next = current
            .evolve(event)
            .map_err(OutboxRepositoryError::InvalidRecord)?;
        self.records.insert(id.clone(), next.clone());
        Ok(next)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OutboxRepositoryError {
    InvalidPageLimit,
    AlreadyExists(OutboxId),
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
            Self::AlreadyExists(id) => write!(f, "outbox record {} already exists", id.key),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(process: &str, key: &str, available_at_ms: i64) -> OutboxRecord {
        OutboxRecord::pending(
            OutboxId {
                process_id: process.into(),
                key: key.into(),
            },
            "topic".into(),
            "{}".into(),
            available_at_ms,
        )
        .unwrap()
    }

    fn claim(fence: u64) -> DeliveryClaim {
        DeliveryClaim {
            owner: "worker".into(),
            fence,
            lease_until_ms: 100,
            heartbeat_revision: 0,
        }
    }

    #[test]
    fn page_is_stable_exclusive_and_time_filtered() {
        let mut repository = MemoryOutboxRepository::default();
        repository.insert(pending("b", "2", 0)).unwrap();
        repository.insert(pending("a", "2", 10)).unwrap();
        repository.insert(pending("a", "1", 0)).unwrap();

        let first = repository.dispatchable_outbox_page(0, None, 1).unwrap();
        assert_eq!(
            first.records[0].id,
            OutboxId {
                process_id: "a".into(),
                key: "1".into()
            }
        );
        let second = repository
            .dispatchable_outbox_page(0, first.next.as_ref(), 1)
            .unwrap();
        assert_eq!(
            second.records[0].id,
            OutboxId {
                process_id: "b".into(),
                key: "2".into()
            }
        );
        assert!(second.next.is_none());
    }

    #[test]
    fn stale_revision_cannot_claim_or_overwrite_record() {
        let record = pending("p", "k", 0);
        let id = record.id.clone();
        let mut repository = MemoryOutboxRepository::default();
        repository.insert(record).unwrap();
        let claimed = repository
            .evolve_outbox(
                &id,
                0,
                OutboxEvent::Claim {
                    claim: claim(1),
                    now_ms: 0,
                },
            )
            .unwrap();
        assert_eq!(claimed.revision, 1);
        assert!(matches!(
            repository.evolve_outbox(
                &id,
                0,
                OutboxEvent::Quarantine {
                    reason: "stale".into()
                }
            ),
            Err(OutboxRepositoryError::RevisionConflict { actual: 1, .. })
        ));
        assert_eq!(repository.load_outbox(&id).unwrap().unwrap(), claimed);
    }
    #[test]
    fn duplicate_insert_is_rejected_without_overwriting_authoritative_record() {
        let original = pending("p", "k", 10);
        let id = original.id.clone();
        let mut repository = MemoryOutboxRepository::default();
        repository.insert(original.clone()).unwrap();

        let conflicting = OutboxRecord::pending(
            id.clone(),
            "different.topic".into(),
            "different-payload".into(),
            20,
        )
        .unwrap();
        assert_eq!(
            repository.insert(conflicting),
            Err(OutboxRepositoryError::AlreadyExists(id.clone())),
        );
        assert_eq!(repository.load_outbox(&id).unwrap(), Some(original));
    }
    #[test]
    fn competing_claims_from_the_same_revision_have_exactly_one_winner() {
        let mut repository = MemoryOutboxRepository::default();
        let record = pending("p", "message", 0);
        let id = record.id.clone();
        repository.insert(record).unwrap();

        let observed_revision = repository.load_outbox(&id).unwrap().unwrap().revision;
        let winner = claim(1);
        let loser = DeliveryClaim {
            owner: "other-worker".into(),
            fence: 1,
            lease_until_ms: 100,
            heartbeat_revision: 0,
        };

        let claimed = repository
            .evolve_outbox(
                &id,
                observed_revision,
                OutboxEvent::Claim {
                    claim: winner.clone(),
                    now_ms: 0,
                },
            )
            .unwrap();
        assert_eq!(claimed.revision, observed_revision + 1);

        assert_eq!(
            repository.evolve_outbox(
                &id,
                observed_revision,
                OutboxEvent::Claim {
                    claim: loser,
                    now_ms: 0,
                },
            ),
            Err(OutboxRepositoryError::RevisionConflict {
                id: id.clone(),
                expected: observed_revision,
                actual: claimed.revision,
            })
        );

        let persisted = repository.load_outbox(&id).unwrap().unwrap();
        assert_eq!(persisted, claimed);
        assert_eq!(
            persisted.state,
            super::super::outbox::OutboxState::Claimed(winner)
        );
        assert_eq!(persisted.attempt, 1);
        assert_eq!(persisted.last_fence, 1);
    }
    #[test]
    fn stale_worker_cannot_confirm_after_expiry_and_higher_fence_reclaim() {
        let mut repository = MemoryOutboxRepository::default();
        let record = pending("p", "message", 0);
        let id = record.id.clone();
        repository.insert(record).unwrap();

        let first_claim = claim(1);
        let first = repository
            .evolve_outbox(
                &id,
                0,
                OutboxEvent::Claim {
                    claim: first_claim.clone(),
                    now_ms: 0,
                },
            )
            .unwrap();
        let released = repository
            .evolve_outbox(
                &id,
                first.revision,
                OutboxEvent::ReleaseExpiredClaim { now_ms: 100 },
            )
            .unwrap();

        let second_claim = DeliveryClaim {
            owner: "new-worker".into(),
            fence: 2,
            lease_until_ms: 250,
            heartbeat_revision: 0,
        };
        let reclaimed = repository
            .evolve_outbox(
                &id,
                released.revision,
                OutboxEvent::Claim {
                    claim: second_claim.clone(),
                    now_ms: 100,
                },
            )
            .unwrap();

        assert!(matches!(
            repository.evolve_outbox(
                &id,
                reclaimed.revision,
                OutboxEvent::ConfirmDelivered {
                    claim: first_claim,
                    receipt: "late-receipt".into(),
                    now_ms: 101,
                },
            ),
            Err(OutboxRepositoryError::InvalidRecord(_))
        ));

        let persisted = repository.load_outbox(&id).unwrap().unwrap();
        assert_eq!(persisted, reclaimed);
        assert_eq!(
            persisted.state,
            super::super::outbox::OutboxState::Claimed(second_claim)
        );
        assert_eq!(persisted.attempt, 2);
        assert_eq!(persisted.last_fence, 2);
    }
}
