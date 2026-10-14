use crate::scheduler::{
    core::commit::aggregate::AggregateSnapshot,
    durable::{
        envelope::{AggregateEnvelope, EnvelopeError},
        snapshot_codec::{SnapshotCodecError, SnapshotCodecV2},
    },
};

use std::fmt;

/// The only supported conversion between a domain aggregate and persisted
/// document bytes. Repositories must not compose envelope and payload codecs
/// themselves because doing so would allow validation steps to drift.
pub(crate) struct AggregateDocumentCodec;

impl AggregateDocumentCodec {
    pub(crate) fn encode(snapshot: &AggregateSnapshot) -> Result<Vec<u8>, AggregateDocumentError> {
        snapshot
            .validate()
            .map_err(SnapshotCodecError::InvalidAggregate)?;
        let payload = SnapshotCodecV2::encode(snapshot)?;
        let sealed = AggregateEnvelope::seal(payload).encode()?;
        Ok(sealed)
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<AggregateSnapshot, AggregateDocumentError> {
        let envelope = AggregateEnvelope::decode(bytes)?;
        let snapshot = SnapshotCodecV2::decode(&envelope.payload)?;
        snapshot
            .validate()
            .map_err(SnapshotCodecError::InvalidAggregate)?;
        Ok(snapshot)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AggregateDocumentError {
    Envelope(EnvelopeError),
    Snapshot(SnapshotCodecError),
}
impl From<EnvelopeError> for AggregateDocumentError {
    fn from(error: EnvelopeError) -> Self {
        Self::Envelope(error)
    }
}
impl From<SnapshotCodecError> for AggregateDocumentError {
    fn from(error: SnapshotCodecError) -> Self {
        Self::Snapshot(error)
    }
}
impl fmt::Display for AggregateDocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "aggregate document error: {self:?}")
    }
}
impl std::error::Error for AggregateDocumentError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::core::commit::aggregate::ExecutionSnapshot;
    use crate::scheduler::core::model::execution_graph::ExecutionId;
    use crate::scheduler::core::model::execution_graph::ExecutionState;
    use crate::scheduler::core::model::model_graph::NodeId;
    use crate::scheduler::core::model::operation::ClaimToken;
    use crate::scheduler::core::model::operation::OperationId;
    use crate::scheduler::core::model::operation::OperationKind;
    use crate::scheduler::core::model::operation::OperationRecord;
    use crate::scheduler::core::model::operation::OperationState;
    use crate::scheduler::core::model::transition_plan::AggregateVersion;
    use std::collections::{BTreeMap, HashMap};

    fn snapshot() -> AggregateSnapshot {
        let execution = ExecutionId::new("step", 2).unwrap();
        let operation_id = OperationId {
            process_id: "process".into(),
            task_id: "step".into(),
            generation: 2,
            sequence: 7,
        };
        let claim = ClaimToken {
            owner: "runtime-a".into(),
            fence: 11,
            lease_until_ms: 9_000,
            heartbeat_revision: 4,
        };
        AggregateSnapshot {
            process_id: "process".into(),
            version: AggregateVersion {
                graph: 2,
                process: 4,
                operations: 8,
            },
            executions: HashMap::from([(
                execution.clone(),
                ExecutionSnapshot {
                    model_node: NodeId::parse("model-step").unwrap(),
                    parent: None,
                    predecessors: vec![],
                    state: ExecutionState::Running,
                    revision: 3,
                },
            )]),
            variables: BTreeMap::from([((execution, "payload".into()), "value".into())]),
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
        let bytes = AggregateDocumentCodec::encode(&source).unwrap();
        assert_eq!(AggregateDocumentCodec::decode(&bytes).unwrap(), source);
    }

    #[test]
    fn corrupted_envelope_is_not_misreported_as_snapshot_failure() {
        let mut bytes = AggregateDocumentCodec::encode(&snapshot()).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        assert!(matches!(
            AggregateDocumentCodec::decode(&bytes),
            Err(AggregateDocumentError::Envelope(
                EnvelopeError::DigestMismatch { .. }
            ))
        ));
    }

    #[test]
    fn valid_envelope_with_invalid_payload_is_reported_as_snapshot_failure() {
        let bytes = AggregateEnvelope::seal(b"not a snapshot".to_vec())
            .encode()
            .unwrap();
        assert_eq!(
            AggregateDocumentCodec::decode(&bytes),
            Err(AggregateDocumentError::Snapshot(
                SnapshotCodecError::InvalidMagic
            )),
        );
    }
}
