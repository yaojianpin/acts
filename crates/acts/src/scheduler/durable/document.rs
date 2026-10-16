use crate::scheduler::{
    core::commit::aggregate::ProcessSnapshot,
    durable::{
        envelope::{EnvelopeError, ProcessEnvelope},
        snapshot_codec::{SnapshotCodecError, SnapshotCodecV2},
    },
};

use std::fmt;

/// The only supported conversion between a domain aggregate and persisted
/// document bytes. Repositories must not compose envelope and payload codecs
/// themselves because doing so would allow validation steps to drift.
pub(crate) struct ProcessDocumentCodec;

impl ProcessDocumentCodec {
    pub(crate) fn encode(snapshot: &ProcessSnapshot) -> Result<Vec<u8>, ProcessDocumentError> {
        snapshot
            .validate()
            .map_err(SnapshotCodecError::InvalidProcess)?;
        let payload = SnapshotCodecV2::encode(snapshot)?;
        let sealed = ProcessEnvelope::seal(payload).encode()?;
        Ok(sealed)
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<ProcessSnapshot, ProcessDocumentError> {
        let envelope = ProcessEnvelope::decode(bytes)?;
        let snapshot = SnapshotCodecV2::decode(&envelope.payload)?;
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
        let bytes = ProcessEnvelope::seal(b"not a snapshot".to_vec())
            .encode()
            .unwrap();
        assert_eq!(
            ProcessDocumentCodec::decode(&bytes),
            Err(ProcessDocumentError::Snapshot(
                SnapshotCodecError::InvalidMagic
            )),
        );
    }
}
