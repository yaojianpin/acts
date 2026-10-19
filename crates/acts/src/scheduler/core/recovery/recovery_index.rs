use crate::scheduler::core::{
    commit::aggregate::ProcessSnapshot,
    model::{operation::OperationId, task_graph::TaskId},
};

use std::collections::HashSet;
use std::fmt;

const MAGIC: &[u8; 8] = b"ACTRIDX\0";
const SCHEMA_V1: u16 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecoveryIndexEntry {
    pub(crate) operation_id: OperationId,
    pub(crate) revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecoveryIndex {
    pub(crate) process_id: String,
    pub(crate) entries: Vec<RecoveryIndexEntry>,
}

impl RecoveryIndex {
    /// The recovery-owned operations: an owner that may be gone, a prepared
    /// commit, a commit waiting to be finalized, or a reconciliation state.
    /// Normal `Pending`/`RetryScheduled` work is in the ready index instead, so
    /// recovery cost tracks exceptional work rather than every parked timer.
    pub(crate) fn project(snapshot: &ProcessSnapshot) -> Option<Self> {
        Self::project_with(snapshot, |operation| operation.needs_recovery())
    }

    /// The pre-A2 projection (every non-terminal operation). A document an
    /// older writer left behind still loads: its aggregate is intact, and the
    /// boot repair rewrites it to [`Self::project`]'s shape.
    pub(crate) fn project_legacy(snapshot: &ProcessSnapshot) -> Option<Self> {
        Self::project_with(snapshot, |operation| !operation.is_terminal())
    }

    fn project_with(
        snapshot: &ProcessSnapshot,
        keep: impl Fn(&crate::scheduler::core::model::operation::OperationRecord) -> bool,
    ) -> Option<Self> {
        let mut entries = snapshot
            .operations
            .values()
            .filter(|operation| keep(operation))
            .map(|operation| RecoveryIndexEntry {
                operation_id: operation.id.clone(),
                revision: operation.revision,
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| operation_order(&left.operation_id, &right.operation_id));
        (!entries.is_empty()).then(|| Self {
            process_id: snapshot.process_id.clone(),
            entries,
        })
    }
}

pub(crate) struct RecoveryIndexCodec;
impl RecoveryIndexCodec {
    pub(crate) fn encode(index: &RecoveryIndex) -> Result<Vec<u8>, RecoveryIndexError> {
        validate(index)?;
        let mut output = Vec::new();
        output.extend_from_slice(MAGIC);
        output.extend_from_slice(&SCHEMA_V1.to_be_bytes());
        write_string(&mut output, &index.process_id)?;
        output.extend_from_slice(
            &u32::try_from(index.entries.len())
                .map_err(|_| RecoveryIndexError::CollectionTooLarge)?
                .to_be_bytes(),
        );
        for entry in &index.entries {
            write_string(&mut output, &entry.operation_id.process_id)?;
            write_string(&mut output, entry.operation_id.task.node.as_str())?;
            output.extend_from_slice(&entry.operation_id.task.generation.to_be_bytes());
            output.extend_from_slice(&entry.operation_id.sequence.to_be_bytes());
            output.extend_from_slice(&entry.revision.to_be_bytes());
        }
        Ok(output)
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<RecoveryIndex, RecoveryIndexError> {
        let mut reader = Reader { bytes, offset: 0 };
        if reader.take(MAGIC.len())? != MAGIC {
            return Err(RecoveryIndexError::InvalidMagic);
        }
        let schema = reader.u16()?;
        if schema != SCHEMA_V1 {
            return Err(RecoveryIndexError::UnsupportedSchema(schema));
        }
        let process_id = reader.string()?;
        let count =
            usize::try_from(reader.u32()?).map_err(|_| RecoveryIndexError::CollectionTooLarge)?;
        // Every entry contains two length prefixes and three u64 values even
        // when both strings are empty. Reject impossible counts before any
        // count-sized allocation.
        const MINIMUM_ENTRY_BYTES: usize = 4 + 4 + 8 + 8 + 8;
        if count > reader.remaining() / MINIMUM_ENTRY_BYTES {
            return Err(RecoveryIndexError::Truncated);
        }
        let mut entries = Vec::with_capacity(count);
        let mut ids = HashSet::with_capacity(count);
        for _ in 0..count {
            let operation_id = OperationId {
                process_id: reader.string()?,
                // The identity type refuses an empty node, so bytes no valid
                // writer can produce are rejected here — the aggregate codec's
                // `InvalidTaskId` discipline (the old pipeline let the empty
                // string through and failed later, in `validate`).
                task: TaskId::new(reader.string()?, reader.u64()?)
                    .map_err(|_| RecoveryIndexError::InvalidTaskId)?,
                sequence: reader.u64()?,
            };
            let revision = reader.u64()?;
            if !ids.insert(operation_id.clone()) {
                return Err(RecoveryIndexError::DuplicateOperation(operation_id));
            }
            entries.push(RecoveryIndexEntry {
                operation_id,
                revision,
            });
        }
        if reader.offset != bytes.len() {
            return Err(RecoveryIndexError::TrailingBytes);
        }
        let index = RecoveryIndex {
            process_id,
            entries,
        };
        validate(&index)?;
        Ok(index)
    }
}

fn validate(index: &RecoveryIndex) -> Result<(), RecoveryIndexError> {
    if index.process_id.trim().is_empty() {
        return Err(RecoveryIndexError::EmptyProcessId);
    }
    if index.entries.is_empty() {
        return Err(RecoveryIndexError::EmptyIndex);
    }
    let mut previous: Option<&OperationId> = None;
    for entry in &index.entries {
        if entry.operation_id.process_id != index.process_id {
            return Err(RecoveryIndexError::ForeignOperation(
                entry.operation_id.clone(),
            ));
        }
        if let Some(previous) = previous {
            match operation_order(previous, &entry.operation_id) {
                std::cmp::Ordering::Less => {}
                std::cmp::Ordering::Equal => {
                    return Err(RecoveryIndexError::DuplicateOperation(
                        entry.operation_id.clone(),
                    ));
                }
                std::cmp::Ordering::Greater => return Err(RecoveryIndexError::NonCanonicalOrder),
            }
        }
        previous = Some(&entry.operation_id);
    }
    Ok(())
}

pub(crate) fn operation_order(left: &OperationId, right: &OperationId) -> std::cmp::Ordering {
    left.process_id
        .cmp(&right.process_id)
        .then_with(|| left.task.node.cmp(&right.task.node))
        .then_with(|| left.task.generation.cmp(&right.task.generation))
        .then_with(|| left.sequence.cmp(&right.sequence))
}
fn write_string(output: &mut Vec<u8>, value: &str) -> Result<(), RecoveryIndexError> {
    output.extend_from_slice(
        &u32::try_from(value.len())
            .map_err(|_| RecoveryIndexError::StringTooLarge)?
            .to_be_bytes(),
    );
    output.extend_from_slice(value.as_bytes());
    Ok(())
}
struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], RecoveryIndexError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(RecoveryIndexError::Truncated)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(RecoveryIndexError::Truncated)?;
        self.offset = end;
        Ok(value)
    }
    fn u16(&mut self) -> Result<u16, RecoveryIndexError> {
        let bytes = self.take(2)?;
        let mut array = [0_u8; 2];
        array.copy_from_slice(bytes);
        Ok(u16::from_be_bytes(array))
    }
    fn u32(&mut self) -> Result<u32, RecoveryIndexError> {
        let bytes = self.take(4)?;
        let mut array = [0_u8; 4];
        array.copy_from_slice(bytes);
        Ok(u32::from_be_bytes(array))
    }
    fn u64(&mut self) -> Result<u64, RecoveryIndexError> {
        let bytes = self.take(8)?;
        let mut array = [0_u8; 8];
        array.copy_from_slice(bytes);
        Ok(u64::from_be_bytes(array))
    }
    fn remaining(&self) -> usize {
        self.bytes.len() - self.offset
    }
    fn string(&mut self) -> Result<String, RecoveryIndexError> {
        let length =
            usize::try_from(self.u32()?).map_err(|_| RecoveryIndexError::StringTooLarge)?;
        String::from_utf8(self.take(length)?.to_vec()).map_err(|_| RecoveryIndexError::InvalidUtf8)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RecoveryIndexError {
    InvalidMagic,
    UnsupportedSchema(u16),
    Truncated,
    TrailingBytes,
    InvalidUtf8,
    EmptyProcessId,
    EmptyIndex,
    /// The bytes name an identity the [`TaskId`] type refuses (an empty
    /// node) — only corrupt or foreign bytes can carry one.
    InvalidTaskId,
    ForeignOperation(OperationId),
    DuplicateOperation(OperationId),
    NonCanonicalOrder,
    StringTooLarge,
    CollectionTooLarge,
}
impl fmt::Display for RecoveryIndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "recovery index error: {self:?}")
    }
}
impl std::error::Error for RecoveryIndexError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded_u32(bytes: &[u8], offset: usize) -> u32 {
        let field = bytes
            .get(offset..offset + 4)
            .expect("test fixture contains the requested u32 field");
        u32::from_be_bytes(field.try_into().expect("test fixture u32 has four bytes"))
    }
    #[test]
    fn codec_round_trip_and_rejects_trailing_bytes() {
        let index = RecoveryIndex {
            process_id: "p".into(),
            entries: vec![RecoveryIndexEntry {
                operation_id: OperationId {
                    process_id: "p".into(),
                    task: TaskId::new("t", 1).unwrap(),
                    sequence: 2,
                },
                revision: 3,
            }],
        };
        let bytes = RecoveryIndexCodec::encode(&index).unwrap();
        assert_eq!(RecoveryIndexCodec::decode(&bytes).unwrap(), index);
        let mut trailing = bytes;
        trailing.push(0);
        assert_eq!(
            RecoveryIndexCodec::decode(&trailing),
            Err(RecoveryIndexError::TrailingBytes)
        );
    }
    fn entry(node: &str, sequence: u64) -> RecoveryIndexEntry {
        RecoveryIndexEntry {
            operation_id: OperationId {
                process_id: "p".into(),
                task: TaskId::new(node, 0).unwrap(),
                sequence,
            },
            revision: sequence,
        }
    }

    #[test]
    fn codec_rejects_non_canonical_order_before_persistence() {
        let index = RecoveryIndex {
            process_id: "p".into(),
            entries: vec![entry("z", 0), entry("a", 0)],
        };
        assert_eq!(
            RecoveryIndexCodec::encode(&index),
            Err(RecoveryIndexError::NonCanonicalOrder)
        );
    }

    #[test]
    fn codec_rejects_duplicate_operations_and_foreign_processes() {
        let duplicate = RecoveryIndex {
            process_id: "p".into(),
            entries: vec![entry("a", 0), entry("a", 0)],
        };
        assert!(matches!(
            RecoveryIndexCodec::encode(&duplicate),
            Err(RecoveryIndexError::DuplicateOperation(_))
        ));

        let mut foreign = entry("a", 0);
        foreign.operation_id.process_id = "other".into();
        let index = RecoveryIndex {
            process_id: "p".into(),
            entries: vec![foreign],
        };
        assert!(matches!(
            RecoveryIndexCodec::encode(&index),
            Err(RecoveryIndexError::ForeignOperation(_))
        ));
    }

    #[test]
    fn decoder_rejects_every_truncated_prefix_without_panicking() {
        let index = RecoveryIndex {
            process_id: "p".into(),
            entries: vec![entry("a", 1)],
        };
        let bytes = RecoveryIndexCodec::encode(&index).unwrap();
        for length in 0..bytes.len() {
            assert_eq!(
                RecoveryIndexCodec::decode(&bytes[..length]),
                Err(RecoveryIndexError::Truncated)
            );
        }
    }
    #[test]
    fn impossible_entry_count_is_rejected_before_allocation() {
        let index = RecoveryIndex {
            process_id: "p".into(),
            entries: vec![entry("t", 1)],
        };
        let mut encoded = RecoveryIndexCodec::encode(&index).unwrap();
        let process_length = usize::try_from(encoded_u32(&encoded, 10)).unwrap();
        let count_offset = 14 + process_length;
        encoded[count_offset..count_offset + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(
            RecoveryIndexCodec::decode(&encoded),
            Err(RecoveryIndexError::Truncated)
        );
    }

    #[test]
    fn decoder_rejects_bad_magic_and_schema() {
        let index = RecoveryIndex {
            process_id: "p".into(),
            entries: vec![entry("a", 1)],
        };
        let bytes = RecoveryIndexCodec::encode(&index).unwrap();
        let mut bad_magic = bytes.clone();
        bad_magic[0] = b'X';
        assert_eq!(
            RecoveryIndexCodec::decode(&bad_magic),
            Err(RecoveryIndexError::InvalidMagic)
        );
        let mut bad_schema = bytes;
        bad_schema[8..10].copy_from_slice(&7u16.to_be_bytes());
        assert_eq!(
            RecoveryIndexCodec::decode(&bad_schema),
            Err(RecoveryIndexError::UnsupportedSchema(7))
        );
    }

    #[test]
    fn decoder_rejects_a_duplicated_entry() {
        let index = RecoveryIndex {
            process_id: "p".into(),
            entries: vec![entry("a", 1)],
        };
        let encoded = RecoveryIndexCodec::encode(&index).unwrap();
        let process_length = usize::try_from(encoded_u32(&encoded, 10)).unwrap();
        let count_offset = 14 + process_length;
        let entry_at = count_offset + 4;
        let entry_bytes = encoded[entry_at..].to_vec();
        let mut duplicated = encoded[..count_offset].to_vec();
        duplicated.extend_from_slice(&2u32.to_be_bytes());
        duplicated.extend_from_slice(&entry_bytes);
        duplicated.extend_from_slice(&entry_bytes);
        assert!(matches!(
            RecoveryIndexCodec::decode(&duplicated),
            Err(RecoveryIndexError::DuplicateOperation(_))
        ));
    }

    #[test]
    fn validation_names_empty_process_and_index() {
        let empty_process = RecoveryIndex {
            process_id: " ".into(),
            entries: vec![entry("a", 0)],
        };
        assert_eq!(
            RecoveryIndexCodec::encode(&empty_process),
            Err(RecoveryIndexError::EmptyProcessId)
        );
        let empty = RecoveryIndex {
            process_id: "p".into(),
            entries: vec![],
        };
        assert_eq!(
            RecoveryIndexCodec::encode(&empty),
            Err(RecoveryIndexError::EmptyIndex)
        );
    }

    #[test]
    fn recovery_index_errors_render() {
        assert!(
            RecoveryIndexError::EmptyIndex
                .to_string()
                .starts_with("recovery index error:")
        );
    }
}
