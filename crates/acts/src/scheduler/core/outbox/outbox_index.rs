use crate::scheduler::core::outbox::records::{OutboxId, OutboxRecord, OutboxState};
use std::collections::{HashMap, HashSet};
use std::fmt;

const MAGIC: &[u8; 8] = b"ACTOIDX\0";
const SCHEMA_V1: u16 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OutboxIndexState {
    Pending { available_at_ms: i64 },
    Claimed { lease_until_ms: i64 },
    DeliveryUnknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OutboxIndexEntry {
    pub(crate) id: OutboxId,
    pub(crate) revision: u64,
    pub(crate) state: OutboxIndexState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OutboxIndex {
    pub(crate) process_id: String,
    pub(crate) entries: Vec<OutboxIndexEntry>,
}

impl OutboxIndex {
    pub(crate) fn project(
        process_id: &str,
        outbox: &HashMap<OutboxId, OutboxRecord>,
    ) -> Result<Option<Self>, OutboxIndexError> {
        let mut entries = Vec::new();
        for (id, record) in outbox {
            if id != &record.id {
                return Err(OutboxIndexError::IdentityMismatch);
            }
            record
                .validate()
                .map_err(|error| OutboxIndexError::InvalidRecord(error.to_string()))?;
            if id.process_id != process_id {
                return Err(OutboxIndexError::ForeignRecord(id.clone()));
            }
            let state = match &record.state {
                OutboxState::Pending { available_at_ms } => OutboxIndexState::Pending {
                    available_at_ms: *available_at_ms,
                },
                OutboxState::Claimed(claim) => OutboxIndexState::Claimed {
                    lease_until_ms: claim.lease_until_ms,
                },
                OutboxState::DeliveryUnknown { .. } => OutboxIndexState::DeliveryUnknown,
                OutboxState::Delivered { .. } | OutboxState::Quarantined { .. } => continue,
            };
            entries.push(OutboxIndexEntry {
                id: id.clone(),
                revision: record.revision,
                state,
            });
        }
        entries.sort_by(|left, right| left.id.cmp(&right.id));
        Ok((!entries.is_empty()).then(|| Self {
            process_id: process_id.into(),
            entries,
        }))
    }
}

pub(crate) struct OutboxIndexCodec;
impl OutboxIndexCodec {
    pub(crate) fn encode(index: &OutboxIndex) -> Result<Vec<u8>, OutboxIndexError> {
        validate(index)?;
        let mut writer = Writer::default();
        writer.raw(MAGIC);
        writer.u16(SCHEMA_V1);
        writer.string(&index.process_id)?;
        writer.count(index.entries.len())?;
        for entry in &index.entries {
            writer.string(&entry.id.process_id)?;
            writer.string(&entry.id.key)?;
            writer.u64(entry.revision);
            match entry.state {
                OutboxIndexState::Pending { available_at_ms } => {
                    writer.u8(0);
                    writer.i64(available_at_ms);
                }
                OutboxIndexState::Claimed { lease_until_ms } => {
                    writer.u8(1);
                    writer.i64(lease_until_ms);
                }
                OutboxIndexState::DeliveryUnknown => writer.u8(2),
            }
        }
        Ok(writer.bytes)
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<OutboxIndex, OutboxIndexError> {
        let mut reader = Reader { bytes, offset: 0 };
        if reader.take(MAGIC.len())? != MAGIC {
            return Err(OutboxIndexError::InvalidMagic);
        }
        let schema = reader.u16()?;
        if schema != SCHEMA_V1 {
            return Err(OutboxIndexError::UnsupportedSchema(schema));
        }
        let process_id = reader.string()?;
        let count =
            usize::try_from(reader.u32()?).map_err(|_| OutboxIndexError::CollectionTooLarge)?;
        let minimum_entry_bytes = 4usize + 4 + 8 + 1;
        if count > reader.remaining() / minimum_entry_bytes {
            return Err(OutboxIndexError::Truncated);
        }
        let mut entries = Vec::with_capacity(count);
        let mut identities = HashSet::with_capacity(count);
        for _ in 0..count {
            let id = OutboxId {
                process_id: reader.string()?,
                key: reader.string()?,
            };
            let revision = reader.u64()?;
            let state = match reader.u8()? {
                0 => OutboxIndexState::Pending {
                    available_at_ms: reader.i64()?,
                },
                1 => OutboxIndexState::Claimed {
                    lease_until_ms: reader.i64()?,
                },
                2 => OutboxIndexState::DeliveryUnknown,
                tag => return Err(OutboxIndexError::UnknownState(tag)),
            };
            if !identities.insert(id.clone()) {
                return Err(OutboxIndexError::DuplicateEntry(id));
            }
            entries.push(OutboxIndexEntry {
                id,
                revision,
                state,
            });
        }
        if !reader.is_finished() {
            return Err(OutboxIndexError::TrailingBytes);
        }
        let index = OutboxIndex {
            process_id,
            entries,
        };
        validate(&index)?;
        Ok(index)
    }
}

fn validate(index: &OutboxIndex) -> Result<(), OutboxIndexError> {
    if index.process_id.trim().is_empty() {
        return Err(OutboxIndexError::EmptyProcessId);
    }
    if index.entries.is_empty() {
        return Err(OutboxIndexError::EmptyIndex);
    }
    let mut previous: Option<&OutboxId> = None;
    for entry in &index.entries {
        if entry.id.process_id != index.process_id {
            return Err(OutboxIndexError::ForeignRecord(entry.id.clone()));
        }
        if entry.id.key.trim().is_empty() {
            return Err(OutboxIndexError::EmptyKey);
        }
        if let Some(previous) = previous {
            match previous.cmp(&entry.id) {
                std::cmp::Ordering::Less => {}
                std::cmp::Ordering::Equal => {
                    return Err(OutboxIndexError::DuplicateEntry(entry.id.clone()));
                }
                std::cmp::Ordering::Greater => return Err(OutboxIndexError::NonCanonicalOrder),
            }
        }
        previous = Some(&entry.id);
    }
    Ok(())
}

#[derive(Default)]
struct Writer {
    bytes: Vec<u8>,
}
impl Writer {
    fn raw(&mut self, value: &[u8]) {
        self.bytes.extend_from_slice(value);
    }
    fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }
    fn u16(&mut self, value: u16) {
        self.raw(&value.to_be_bytes());
    }
    fn u32(&mut self, value: u32) {
        self.raw(&value.to_be_bytes());
    }
    fn u64(&mut self, value: u64) {
        self.raw(&value.to_be_bytes());
    }
    fn i64(&mut self, value: i64) {
        self.raw(&value.to_be_bytes());
    }
    fn count(&mut self, value: usize) -> Result<(), OutboxIndexError> {
        self.u32(u32::try_from(value).map_err(|_| OutboxIndexError::CollectionTooLarge)?);
        Ok(())
    }
    fn string(&mut self, value: &str) -> Result<(), OutboxIndexError> {
        self.count(value.len())?;
        self.raw(value.as_bytes());
        Ok(())
    }
}
struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Reader<'a> {
    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }
    fn is_finished(&self) -> bool {
        self.offset == self.bytes.len()
    }
    fn take(&mut self, length: usize) -> Result<&'a [u8], OutboxIndexError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(OutboxIndexError::Truncated)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(OutboxIndexError::Truncated)?;
        self.offset = end;
        Ok(value)
    }
    fn u8(&mut self) -> Result<u8, OutboxIndexError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, OutboxIndexError> {
        let bytes = self.take(2)?;
        let mut array = [0_u8; 2];
        array.copy_from_slice(bytes);
        Ok(u16::from_be_bytes(array))
    }
    fn u32(&mut self) -> Result<u32, OutboxIndexError> {
        let bytes = self.take(4)?;
        let mut array = [0_u8; 4];
        array.copy_from_slice(bytes);
        Ok(u32::from_be_bytes(array))
    }
    fn u64(&mut self) -> Result<u64, OutboxIndexError> {
        let bytes = self.take(8)?;
        let mut array = [0_u8; 8];
        array.copy_from_slice(bytes);
        Ok(u64::from_be_bytes(array))
    }
    fn i64(&mut self) -> Result<i64, OutboxIndexError> {
        let bytes = self.take(8)?;
        let mut array = [0_u8; 8];
        array.copy_from_slice(bytes);
        Ok(i64::from_be_bytes(array))
    }
    fn string(&mut self) -> Result<String, OutboxIndexError> {
        let length =
            usize::try_from(self.u32()?).map_err(|_| OutboxIndexError::CollectionTooLarge)?;
        String::from_utf8(self.take(length)?.to_vec()).map_err(|_| OutboxIndexError::InvalidUtf8)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OutboxIndexError {
    InvalidMagic,
    UnsupportedSchema(u16),
    Truncated,
    TrailingBytes,
    InvalidUtf8,
    EmptyProcessId,
    EmptyKey,
    EmptyIndex,
    IdentityMismatch,
    ForeignRecord(OutboxId),
    DuplicateEntry(OutboxId),
    NonCanonicalOrder,
    UnknownState(u8),
    CollectionTooLarge,
    InvalidRecord(String),
}
impl fmt::Display for OutboxIndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "outbox index error: {self:?}")
    }
}
impl std::error::Error for OutboxIndexError {}

#[cfg(test)]
mod tests {
    fn encoded_u32(bytes: &[u8], offset: usize) -> u32 {
        let mut value = [0_u8; 4];
        value.copy_from_slice(&bytes[offset..offset + 4]);
        u32::from_be_bytes(value)
    }

    use super::*;
    use crate::scheduler::core::outbox::records::{DeliveryClaim, OutboxEvent};
    fn pending(key: &str) -> OutboxRecord {
        OutboxRecord::pending(
            OutboxId {
                process_id: "p".into(),
                key: key.into(),
            },
            "topic".into(),
            "{}".into(),
            17,
        )
        .unwrap()
    }
    #[test]
    fn projection_omits_terminal_records_and_codec_round_trips() {
        let mut records = HashMap::new();
        let pending_record = pending("b");
        records.insert(pending_record.id.clone(), pending_record);
        let delivered = pending("a");
        let claim = DeliveryClaim {
            owner: "w".into(),
            fence: 1,
            lease_until_ms: 18,
            heartbeat_revision: 0,
        };
        let delivered = delivered
            .evolve(OutboxEvent::Claim {
                claim: claim.clone(),
                now_ms: 17,
            })
            .unwrap()
            .evolve(OutboxEvent::ConfirmDelivered {
                claim,
                receipt: "ok".into(),
                now_ms: 0,
            })
            .unwrap();
        records.insert(delivered.id.clone(), delivered);
        let index = OutboxIndex::project("p", &records).unwrap().unwrap();
        assert_eq!(index.entries.len(), 1);
        assert_eq!(index.entries[0].id.key, "b");
        let bytes = OutboxIndexCodec::encode(&index).unwrap();
        assert_eq!(OutboxIndexCodec::decode(&bytes).unwrap(), index);
    }
    #[test]
    fn decoder_rejects_every_truncated_prefix() {
        let record = pending("a");
        let index = OutboxIndex::project("p", &HashMap::from([(record.id.clone(), record)]))
            .unwrap()
            .unwrap();
        let bytes = OutboxIndexCodec::encode(&index).unwrap();
        for length in 0..bytes.len() {
            assert_eq!(
                OutboxIndexCodec::decode(&bytes[..length]),
                Err(OutboxIndexError::Truncated)
            );
        }
    }
    #[test]
    fn decoder_rejects_trailing_bytes_and_unknown_state() {
        let record = pending("a");
        let index = OutboxIndex::project("p", &HashMap::from([(record.id.clone(), record)]))
            .unwrap()
            .unwrap();
        let mut bytes = OutboxIndexCodec::encode(&index).unwrap();
        let state_offset = bytes.len() - 9;
        bytes[state_offset] = 99;
        assert_eq!(
            OutboxIndexCodec::decode(&bytes),
            Err(OutboxIndexError::UnknownState(99))
        );
        let mut valid = OutboxIndexCodec::encode(&index).unwrap();
        valid.push(0);
        assert_eq!(
            OutboxIndexCodec::decode(&valid),
            Err(OutboxIndexError::TrailingBytes)
        );
    }
    #[test]
    fn impossible_entry_count_is_rejected_before_allocation() {
        let record = pending("a");
        let index = OutboxIndex::project("p", &HashMap::from([(record.id.clone(), record)]))
            .unwrap()
            .unwrap();
        let mut encoded = OutboxIndexCodec::encode(&index).unwrap();
        let process_length = usize::try_from(encoded_u32(&encoded, 10)).unwrap();
        let count_offset = 14 + process_length;
        encoded[count_offset..count_offset + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(
            OutboxIndexCodec::decode(&encoded),
            Err(OutboxIndexError::Truncated)
        );
    }

    #[test]
    fn projection_names_identity_and_foreign_process_mistakes() {
        let record = pending("a");
        // The map key disagrees with the record's own identity.
        let mismatched = OutboxIndex::project(
            "p",
            &HashMap::from([(
                OutboxId {
                    process_id: "p".into(),
                    key: "other".into(),
                },
                record.clone(),
            )]),
        );
        assert_eq!(mismatched, Err(OutboxIndexError::IdentityMismatch));

        // The record belongs to another process document.
        assert!(matches!(
            OutboxIndex::project("other", &HashMap::from([(record.id.clone(), record)])),
            Err(OutboxIndexError::ForeignRecord(_))
        ));
    }

    #[test]
    fn projection_of_only_terminal_records_is_none() {
        let record = pending("a");
        let delivered = record
            .evolve(OutboxEvent::Claim {
                claim: DeliveryClaim {
                    owner: "w".into(),
                    fence: 1,
                    lease_until_ms: 18,
                    heartbeat_revision: 0,
                },
                now_ms: 17,
            })
            .unwrap()
            .evolve(OutboxEvent::ConfirmDelivered {
                claim: DeliveryClaim {
                    owner: "w".into(),
                    fence: 1,
                    lease_until_ms: 18,
                    heartbeat_revision: 0,
                },
                receipt: "ok".into(),
                now_ms: 17,
            })
            .unwrap();
        assert!(
            OutboxIndex::project("p", &HashMap::from([(delivered.id.clone(), delivered)]))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn decoder_rejects_bad_magic_and_schema() {
        let record = pending("a");
        let index = OutboxIndex::project("p", &HashMap::from([(record.id.clone(), record)]))
            .unwrap()
            .unwrap();
        let bytes = OutboxIndexCodec::encode(&index).unwrap();
        let mut bad_magic = bytes.clone();
        bad_magic[0] = b'X';
        assert_eq!(
            OutboxIndexCodec::decode(&bad_magic),
            Err(OutboxIndexError::InvalidMagic)
        );
        let mut bad_schema = bytes;
        bad_schema[8..10].copy_from_slice(&3u16.to_be_bytes());
        assert_eq!(
            OutboxIndexCodec::decode(&bad_schema),
            Err(OutboxIndexError::UnsupportedSchema(3))
        );
    }

    #[test]
    fn claimed_entries_round_trip_through_the_codec() {
        let pending_record = pending("a");
        let claimed = pending_record
            .evolve(OutboxEvent::Claim {
                claim: DeliveryClaim {
                    owner: "w".into(),
                    fence: 1,
                    lease_until_ms: 50,
                    heartbeat_revision: 0,
                },
                now_ms: 17,
            })
            .unwrap();
        let index = OutboxIndex::project("p", &HashMap::from([(claimed.id.clone(), claimed)]))
            .unwrap()
            .unwrap();
        assert_eq!(
            index.entries[0].state,
            OutboxIndexState::Claimed { lease_until_ms: 50 }
        );
        let bytes = OutboxIndexCodec::encode(&index).unwrap();
        assert_eq!(OutboxIndexCodec::decode(&bytes).unwrap(), index);
    }

    #[test]
    fn decoder_rejects_a_duplicated_entry() {
        let record = pending("a");
        let index = OutboxIndex::project("p", &HashMap::from([(record.id.clone(), record)]))
            .unwrap()
            .unwrap();
        let encoded = OutboxIndexCodec::encode(&index).unwrap();
        let process_length = usize::try_from(encoded_u32(&encoded, 10)).unwrap();
        let count_offset = 14 + process_length;
        let entry_at = count_offset + 4;
        let entry_bytes = encoded[entry_at..].to_vec();
        let mut duplicated = encoded[..count_offset].to_vec();
        duplicated.extend_from_slice(&2u32.to_be_bytes());
        duplicated.extend_from_slice(&entry_bytes);
        duplicated.extend_from_slice(&entry_bytes);
        assert!(matches!(
            OutboxIndexCodec::decode(&duplicated),
            Err(OutboxIndexError::DuplicateEntry(_))
        ));
    }

    #[test]
    fn validation_names_each_structural_mistake() {
        let empty_process = OutboxIndex {
            process_id: " ".into(),
            entries: vec![OutboxIndexEntry {
                id: OutboxId {
                    process_id: "p".into(),
                    key: "k".into(),
                },
                revision: 0,
                state: OutboxIndexState::DeliveryUnknown,
            }],
        };
        assert_eq!(
            OutboxIndexCodec::encode(&empty_process),
            Err(OutboxIndexError::EmptyProcessId)
        );

        let empty = OutboxIndex {
            process_id: "p".into(),
            entries: vec![],
        };
        assert_eq!(
            OutboxIndexCodec::encode(&empty),
            Err(OutboxIndexError::EmptyIndex)
        );

        let foreign = OutboxIndex {
            process_id: "p".into(),
            entries: vec![OutboxIndexEntry {
                id: OutboxId {
                    process_id: "other".into(),
                    key: "k".into(),
                },
                revision: 0,
                state: OutboxIndexState::DeliveryUnknown,
            }],
        };
        assert!(matches!(
            OutboxIndexCodec::encode(&foreign),
            Err(OutboxIndexError::ForeignRecord(_))
        ));

        let empty_key = OutboxIndex {
            process_id: "p".into(),
            entries: vec![OutboxIndexEntry {
                id: OutboxId {
                    process_id: "p".into(),
                    key: " ".into(),
                },
                revision: 0,
                state: OutboxIndexState::DeliveryUnknown,
            }],
        };
        assert_eq!(
            OutboxIndexCodec::encode(&empty_key),
            Err(OutboxIndexError::EmptyKey)
        );

        let entry = |key: &str| OutboxIndexEntry {
            id: OutboxId {
                process_id: "p".into(),
                key: key.into(),
            },
            revision: 0,
            state: OutboxIndexState::DeliveryUnknown,
        };
        let duplicate = OutboxIndex {
            process_id: "p".into(),
            entries: vec![entry("a"), entry("a")],
        };
        assert!(matches!(
            OutboxIndexCodec::encode(&duplicate),
            Err(OutboxIndexError::DuplicateEntry(_))
        ));
        let unordered = OutboxIndex {
            process_id: "p".into(),
            entries: vec![entry("b"), entry("a")],
        };
        assert_eq!(
            OutboxIndexCodec::encode(&unordered),
            Err(OutboxIndexError::NonCanonicalOrder)
        );
    }

    #[test]
    fn outbox_index_errors_render() {
        assert!(
            OutboxIndexError::EmptyIndex
                .to_string()
                .starts_with("outbox index error:")
        );
    }
}
