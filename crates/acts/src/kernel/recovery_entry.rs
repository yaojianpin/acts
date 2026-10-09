use super::{AggregateSnapshot, OperationId};
use std::collections::BTreeMap;
use std::fmt;

pub(super) const RECOVERY_ENTRY_KEY_PREFIX: &str = "kernel/indexes/recovery-entries/";
const COMPONENT_SEPARATOR: char = '.';
const REVISION_BYTES: usize = 8;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RecoveryEntryDocument {
    pub(super) operation_id: OperationId,
    pub(super) revision: u64,
}

impl RecoveryEntryDocument {
    pub(super) fn project(snapshot: &AggregateSnapshot) -> BTreeMap<String, Vec<u8>> {
        snapshot
            .operations
            .values()
            .filter(|operation| !operation.is_terminal())
            .map(|operation| {
                (
                    recovery_entry_key(&operation.id),
                    encode_revision(operation.revision),
                )
            })
            .collect()
    }

    pub(super) fn decode(key: &str, value: &[u8]) -> Result<Self, RecoveryEntryError> {
        Ok(Self {
            operation_id: decode_recovery_entry_key(key)?,
            revision: decode_revision(value)?,
        })
    }
}

pub(super) fn recovery_entry_key(id: &OperationId) -> String {
    let mut key = String::with_capacity(
        RECOVERY_ENTRY_KEY_PREFIX.len()
            + id.process_id.len() * 2
            + id.task_id.len() * 2
            + 2
            + 16
            + 1
            + 16,
    );
    key.push_str(RECOVERY_ENTRY_KEY_PREFIX);
    push_hex(&mut key, id.process_id.as_bytes());
    key.push(COMPONENT_SEPARATOR);
    push_hex(&mut key, id.task_id.as_bytes());
    key.push(COMPONENT_SEPARATOR);
    key.push_str(&format_u64(id.generation));
    key.push(COMPONENT_SEPARATOR);
    key.push_str(&format_u64(id.sequence));
    key
}

pub(super) fn recovery_entry_after_key(id: &OperationId) -> String {
    let mut key = recovery_entry_key(id);
    key.push('\0');
    key
}

fn decode_recovery_entry_key(key: &str) -> Result<OperationId, RecoveryEntryError> {
    let suffix = key
        .strip_prefix(RECOVERY_ENTRY_KEY_PREFIX)
        .ok_or(RecoveryEntryError::InvalidKey)?;
    let mut parts = suffix.split(COMPONENT_SEPARATOR);
    let process_id = decode_string(parts.next().ok_or(RecoveryEntryError::InvalidKey)?)?;
    let task_id = decode_string(parts.next().ok_or(RecoveryEntryError::InvalidKey)?)?;
    let generation = decode_u64(parts.next().ok_or(RecoveryEntryError::InvalidKey)?)?;
    let sequence = decode_u64(parts.next().ok_or(RecoveryEntryError::InvalidKey)?)?;
    if parts.next().is_some() {
        return Err(RecoveryEntryError::InvalidKey);
    }
    Ok(OperationId {
        process_id,
        task_id,
        generation,
        sequence,
    })
}

fn encode_revision(revision: u64) -> Vec<u8> {
    revision.to_be_bytes().to_vec()
}

fn decode_revision(value: &[u8]) -> Result<u64, RecoveryEntryError> {
    if value.len() != REVISION_BYTES {
        return Err(RecoveryEntryError::InvalidValue);
    }
    let mut bytes = [0_u8; REVISION_BYTES];
    bytes.copy_from_slice(value);
    Ok(u64::from_be_bytes(bytes))
}

fn push_hex(output: &mut String, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
}

fn decode_string(value: &str) -> Result<String, RecoveryEntryError> {
    if !value.len().is_multiple_of(2) {
        return Err(RecoveryEntryError::InvalidKey);
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().as_chunks::<2>().0 {
        bytes.push((decode_nibble(pair[0])? << 4) | decode_nibble(pair[1])?);
    }
    String::from_utf8(bytes).map_err(|_| RecoveryEntryError::InvalidKey)
}

fn decode_nibble(value: u8) -> Result<u8, RecoveryEntryError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err(RecoveryEntryError::InvalidKey),
    }
}

fn format_u64(value: u64) -> String {
    format!("{value:016x}")
}

fn decode_u64(value: &str) -> Result<u64, RecoveryEntryError> {
    if value.len() != 16 {
        return Err(RecoveryEntryError::InvalidKey);
    }
    u64::from_str_radix(value, 16).map_err(|_| RecoveryEntryError::InvalidKey)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RecoveryEntryError {
    InvalidKey,
    InvalidValue,
}

impl fmt::Display for RecoveryEntryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidKey => write!(f, "invalid recovery entry key"),
            Self::InvalidValue => write!(f, "invalid recovery entry value"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(process_id: &str, task_id: &str, generation: u64, sequence: u64) -> OperationId {
        OperationId {
            process_id: process_id.into(),
            task_id: task_id.into(),
            generation,
            sequence,
        }
    }

    #[test]
    fn key_round_trip_preserves_unicode_identity() {
        let expected = id("流程", "task/α", 12, 34);
        let key = recovery_entry_key(&expected);
        assert_eq!(decode_recovery_entry_key(&key), Ok(expected));
    }

    #[test]
    fn key_order_matches_operation_order() {
        let mut ids = vec![
            id("p", "b", 0, 0),
            id("p", "a", 2, 0),
            id("p", "a", 1, 9),
            id("p", "a", 1, 1),
            id("p2", "a", 0, 0),
        ];
        let mut keys = ids
            .iter()
            .map(|id| (recovery_entry_key(id), id.clone()))
            .collect::<Vec<_>>();
        ids.sort_by(super::super::recovery_index::operation_order);
        keys.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(keys.into_iter().map(|(_, id)| id).collect::<Vec<_>>(), ids);
    }

    #[test]
    fn after_key_is_exclusive_and_before_the_next_identity() {
        let current = id("p", "a", 1, 1);
        let next = id("p", "a", 1, 2);
        assert!(recovery_entry_key(&current) < recovery_entry_after_key(&current));
        assert!(recovery_entry_after_key(&current) < recovery_entry_key(&next));
    }

    #[test]
    fn document_round_trip_preserves_revision() {
        let operation_id = id("p", "t", u64::MAX, 7);
        let key = recovery_entry_key(&operation_id);
        let value = encode_revision(42);
        assert_eq!(
            RecoveryEntryDocument::decode(&key, &value),
            Ok(RecoveryEntryDocument {
                operation_id,
                revision: 42,
            })
        );
    }

    #[test]
    fn malformed_key_and_value_are_rejected() {
        assert_eq!(
            RecoveryEntryDocument::decode("wrong", &[0; 8]),
            Err(RecoveryEntryError::InvalidKey)
        );
        let key = recovery_entry_key(&id("p", "t", 0, 0));
        assert_eq!(
            RecoveryEntryDocument::decode(&key, &[0; 7]),
            Err(RecoveryEntryError::InvalidValue)
        );
    }
}
