use crate::scheduler::{
    core::commit::aggregate::ProcessSnapshot,
    durable::{
        envelope::{EnvelopeError, ProcessEnvelope},
        snapshot_codec::{SnapshotCodecError, SnapshotCodecV2, SnapshotCodecV3},
    },
};

use std::fmt;

/// The only supported conversion between a domain aggregate and persisted
/// document bytes. Repositories must not compose envelope and payload codecs
/// themselves because doing so would allow validation steps to drift.
///
/// Writes are V3 (the versioned identity: every instance carries its wire).
/// Reads accept V2 as well — the C5 upcast: a V2 document holds the same
/// node strings and generations, so its instances derive the identical wire
/// (`instance id := 该代 node 的 wire`, degenerate equivalence, behavior
/// unchanged), and nothing else in the layout moved.
pub(crate) struct ProcessDocumentCodec;

impl ProcessDocumentCodec {
    pub(crate) fn encode(snapshot: &ProcessSnapshot) -> Result<Vec<u8>, ProcessDocumentError> {
        snapshot
            .validate()
            .map_err(SnapshotCodecError::InvalidProcess)?;
        let payload = SnapshotCodecV3::encode(snapshot)?;
        let sealed = ProcessEnvelope::seal(payload)?;
        Ok(sealed)
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<ProcessSnapshot, ProcessDocumentError> {
        let envelope = ProcessEnvelope::decode(bytes)?;
        let snapshot = match SnapshotCodecV3::decode(&envelope.payload) {
            Err(SnapshotCodecError::InvalidMagic) => SnapshotCodecV2::decode(&envelope.payload)?,
            other => other?,
        };
        snapshot
            .validate()
            .map_err(SnapshotCodecError::InvalidProcess)?;
        Ok(snapshot)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ProcessDocumentError {
    Envelope(EnvelopeError),
    Snapshot(SnapshotCodecError),
}
impl From<EnvelopeError> for ProcessDocumentError {
    fn from(error: EnvelopeError) -> Self {
        Self::Envelope(error)
    }
}
impl From<SnapshotCodecError> for ProcessDocumentError {
    fn from(error: SnapshotCodecError) -> Self {
        Self::Snapshot(error)
    }
}
impl fmt::Display for ProcessDocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "aggregate document error: {self:?}")
    }
}
impl std::error::Error for ProcessDocumentError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::core::commit::aggregate::TaskSnapshot;
    use crate::scheduler::core::model::model_graph::NodeId;
    use crate::scheduler::core::model::operation::ClaimToken;
    use crate::scheduler::core::model::operation::OperationId;
    use crate::scheduler::core::model::operation::OperationKind;
    use crate::scheduler::core::model::operation::OperationRecord;
    use crate::scheduler::core::model::operation::OperationState;
    use crate::scheduler::core::model::task_graph::TaskId;
    use crate::scheduler::core::model::task_graph::TaskState;
    use crate::scheduler::core::model::transition_plan::ProcessVersion;
    use std::collections::{BTreeMap, HashMap};

    fn snapshot() -> ProcessSnapshot {
        let task = TaskId::new("step", 2).unwrap();
        let operation_id = OperationId {
            process_id: "process".into(),
            task: task.clone(),
            sequence: 7,
        };
        let claim = ClaimToken {
            owner: "runtime-a".into(),
            fence: 11,
            lease_until_ms: 9_000,
            heartbeat_revision: 4,
        };
        ProcessSnapshot {
            process_id: "process".into(),
            version: ProcessVersion {
                graph: 2,
                process: 4,
                operations: 8,
            },
            tasks: HashMap::from([(
                task.clone(),
                TaskSnapshot {
                    model_node: NodeId::parse("model-step").unwrap(),
                    parent: None,
                    predecessors: vec![],
                    state: TaskState::Running,
                    revision: 3,
                },
            )]),
            variables: BTreeMap::from([((task, "payload".into()), "value".into())]),
            operations: HashMap::from([(
                operation_id.clone(),
                OperationRecord {
                    id: operation_id,
                    kind: OperationKind::ExecuteTask,
                    state: OperationState::Claimed(claim),
                    revision: 5,
                },
            )]),
            outbox: HashMap::new(),
        }
    }

    #[test]
    fn document_round_trip_crosses_every_validation_layer() {
        let source = snapshot();
        let bytes = ProcessDocumentCodec::encode(&source).unwrap();
        assert_eq!(ProcessDocumentCodec::decode(&bytes).unwrap(), source);
    }

    /// The single hash on the write path is deterministic: the same snapshot
    /// always seals to the same document, and the header digest it carries is
    /// the digest of the payload it carries.
    #[test]
    fn document_encoding_is_byte_stable() {
        use crate::scheduler::core::commit::aggregate::sha256_hex;

        let source = snapshot();
        let first = ProcessDocumentCodec::encode(&source).unwrap();
        let second = ProcessDocumentCodec::encode(&source).unwrap();
        assert_eq!(first, second);

        let envelope = ProcessEnvelope::decode(&first).unwrap();
        assert_eq!(envelope.payload_digest, sha256_hex(&envelope.payload));
        assert_eq!(ProcessDocumentCodec::decode(&first).unwrap(), source);
    }

    #[test]
    fn corrupted_envelope_is_not_misreported_as_snapshot_failure() {
        let mut bytes = ProcessDocumentCodec::encode(&snapshot()).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        assert!(matches!(
            ProcessDocumentCodec::decode(&bytes),
            Err(ProcessDocumentError::Envelope(
                EnvelopeError::DigestMismatch { .. }
            ))
        ));
    }

    #[test]
    fn valid_envelope_with_invalid_payload_is_reported_as_snapshot_failure() {
        let bytes = ProcessEnvelope::seal(b"not a snapshot".to_vec()).unwrap();
        assert_eq!(
            ProcessDocumentCodec::decode(&bytes),
            Err(ProcessDocumentError::Snapshot(
                SnapshotCodecError::InvalidMagic
            )),
        );
    }

    /// The C5 upcast, at the boundary production loads through: a document an
    /// earlier kernel wrote (V2 payload — node string + generation, no wire)
    /// still decodes, and its instances derive the identical wire, so the
    /// loaded aggregate is value-equal to what a V3 write would produce.
    #[test]
    fn v2_documents_upcast_to_the_loaded_snapshot() {
        use crate::scheduler::durable::snapshot_codec::SnapshotCodecV2;
        let source = snapshot();
        let v2 = ProcessEnvelope::seal(SnapshotCodecV2::encode(&source).unwrap()).unwrap();
        assert_eq!(ProcessDocumentCodec::decode(&v2).unwrap(), source);
    }
}
