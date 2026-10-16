use crate::scheduler::core::{commit::aggregate::ProcessSnapshot, model::operation::OperationId};

use std::collections::BTreeMap;

pub(crate) const RECOVERY_ENTRY_KEY_PREFIX: &str = "kernel/indexes/recovery-entries/";
const COMPONENT_SEPARATOR: char = '.';

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecoveryEntryDocument;

impl RecoveryEntryDocument {
    pub(crate) fn project(snapshot: &ProcessSnapshot) -> BTreeMap<String, Vec<u8>> {
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
}

pub(crate) fn recovery_entry_key(id: &OperationId) -> String {
    let mut key = String::with_capacity(
        RECOVERY_ENTRY_KEY_PREFIX.len()
            + id.process_id.len() * 2
            + id.node.len() * 2
            + 2
            + 16
            + 1
            + 16,
    );
    key.push_str(RECOVERY_ENTRY_KEY_PREFIX);
    push_hex(&mut key, id.process_id.as_bytes());
    key.push(COMPONENT_SEPARATOR);
    push_hex(&mut key, id.node.as_bytes());
    key.push(COMPONENT_SEPARATOR);
    key.push_str(&format_u64(id.generation));
    key.push(COMPONENT_SEPARATOR);
    key.push_str(&format_u64(id.sequence));
    key
}

fn encode_revision(revision: u64) -> Vec<u8> {
    revision.to_be_bytes().to_vec()
}

fn push_hex(output: &mut String, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
}

fn format_u64(value: u64) -> String {
    format!("{value:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(process_id: &str, node: &str, generation: u64, sequence: u64) -> OperationId {
        OperationId {
            process_id: process_id.into(),
            node: node.into(),
            generation,
            sequence,
        }
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
        ids.sort_by(crate::scheduler::core::recovery::recovery_index::operation_order);
        keys.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(keys.into_iter().map(|(_, id)| id).collect::<Vec<_>>(), ids);
    }
}
