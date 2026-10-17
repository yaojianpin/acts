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
    // C5 (design §3.1): the instance component is the identity's wire —
    // `sha256(pid, node, gen)[..32]` — so the key is constant-length and the
    // generation no longer appears as its own component (the wire already
    // binds it). The wire is pure hex, so it rides the key unescaped between
    // the two hex components. Keys written before C5 (`hex(pid).hex(node).gen
    // .seq`) are orphans: the recovery scan reads only the new shape, and the
    // orphan policy is registered with §11 L1.
    let wire = id.task.wire(&id.process_id);
    let mut key = String::with_capacity(
        RECOVERY_ENTRY_KEY_PREFIX.len() + id.process_id.len() * 2 + 1 + wire.len() + 1 + 16,
    );
    key.push_str(RECOVERY_ENTRY_KEY_PREFIX);
    push_hex(&mut key, id.process_id.as_bytes());
    key.push(COMPONENT_SEPARATOR);
    key.push_str(&wire);
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
            task: crate::scheduler::core::model::task_graph::TaskId::new(node, generation).unwrap(),
            sequence,
        }
    }

    #[test]
    fn keys_are_injective_and_stable_per_operation() {
        // C5: the entry key sorts by the wire (a digest), not by the
        // operation order any more — the property worth pinning is that the
        // key still separates every (process, node, generation, sequence)
        // and is stable, so a prefix scan finds each live operation exactly
        // once.
        let ids = [
            id("p", "b", 0, 0),
            id("p", "a", 2, 0),
            id("p", "a", 1, 9),
            id("p", "a", 1, 1),
            id("p2", "a", 0, 0),
        ];
        let keys = ids.iter().map(recovery_entry_key).collect::<Vec<_>>();
        let mut unique = keys.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), keys.len(), "every operation owns its key");
        for (id, key) in ids.iter().zip(&keys) {
            assert_eq!(&recovery_entry_key(id), key, "the key is stable");
        }
        // The instance component is the constant-length wire (32 hex), so
        // keys of one process differ only in the digest and sequence slots.
        for key in &keys {
            assert!(
                key.starts_with(RECOVERY_ENTRY_KEY_PREFIX),
                "{key} carries the recovery prefix"
            );
        }
    }
}
