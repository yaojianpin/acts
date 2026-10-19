use crate::scheduler::{
    core::commit::aggregate::ProcessSnapshot,
    durable::{
        envelope::{EnvelopeError, ProcessEnvelope},
        snapshot_codec::{
            HeadPayload, LogChanges, LogPayload, SnapshotCodecError, SnapshotCodecV2,
            SnapshotCodecV3, SnapshotCodecV4,
        },
    },
};

use std::fmt;

/// The legacy (pre-L8) whole-snapshot codec. It is the only reader of the
/// V2/V3 documents an earlier kernel wrote; production writes the L8 family
/// through [`ProcessLogCodec`] instead.
pub(crate) struct ProcessDocumentCodec;

impl ProcessDocumentCodec {
    /// The legacy writer, kept as the migration fixture builder. Production
    /// never writes a whole aggregate document any more.
    #[cfg(test)]
    pub(crate) fn encode(snapshot: &ProcessSnapshot) -> Result<Vec<u8>, ProcessDocumentError> {
        snapshot
            .validate()
            .map_err(SnapshotCodecError::InvalidProcess)?;
        let payload = SnapshotCodecV3::encode(snapshot)?;
        let sealed = ProcessEnvelope::seal(payload)?;
        Ok(sealed)
    }

    #[cfg(test)]
    pub(crate) fn decode(bytes: &[u8]) -> Result<ProcessSnapshot, ProcessDocumentError> {
        let envelope = ProcessEnvelope::decode(bytes)?;
        if envelope.schema_version
            != crate::scheduler::durable::envelope::LEGACY_AGGREGATE_SCHEMA_VERSION
        {
            return Err(EnvelopeError::UnsupportedSchema(envelope.schema_version).into());
        }
        Self::decode_payload(&envelope.payload)
    }

    /// Decode a legacy payload (envelope already verified).
    pub(crate) fn decode_payload(payload: &[u8]) -> Result<ProcessSnapshot, ProcessDocumentError> {
        let snapshot = match SnapshotCodecV3::decode(payload) {
            Err(SnapshotCodecError::InvalidMagic) => SnapshotCodecV2::decode(payload)?,
            other => other?,
        };
        snapshot
            .validate()
            .map_err(SnapshotCodecError::InvalidProcess)?;
        Ok(snapshot)
    }
}

/// The L8 document family: one small head plus a base snapshot and an
/// append-only log of per-commit records.
///
/// A normal commit encodes only the records it changed (one log entry) and a
/// fresh head; it never rewrites the whole aggregate. The base snapshot is
/// rewritten only when the log reaches its threshold, and the whole family is
/// written in the same atomic guarded batch as the derived indexes. Reads
/// accept the legacy whole-snapshot documents too (see
/// [`ProcessDocumentCodec`]).
pub(crate) struct ProcessLogCodec;

impl ProcessLogCodec {
    pub(crate) fn encode_head(head: &HeadPayload) -> Result<Vec<u8>, ProcessDocumentError> {
        let payload = SnapshotCodecV4::encode_head(head)?;
        ProcessEnvelope::seal_v2(payload).map_err(Into::into)
    }

    #[cfg(test)]
    pub(crate) fn decode_head(bytes: &[u8]) -> Result<HeadPayload, ProcessDocumentError> {
        let envelope = ProcessEnvelope::decode(bytes)?;
        if envelope.schema_version != crate::scheduler::durable::envelope::AGGREGATE_SCHEMA_VERSION
        {
            return Err(EnvelopeError::UnsupportedSchema(envelope.schema_version).into());
        }
        Self::decode_head_payload(&envelope.payload)
    }

    pub(crate) fn decode_head_payload(payload: &[u8]) -> Result<HeadPayload, ProcessDocumentError> {
        SnapshotCodecV4::decode_head(payload).map_err(Into::into)
    }

    pub(crate) fn encode_base(
        snapshot: &ProcessSnapshot,
        base_seq: u64,
    ) -> Result<Vec<u8>, ProcessDocumentError> {
        let payload = SnapshotCodecV4::encode_base(snapshot, base_seq)?;
        ProcessEnvelope::seal_v2(payload).map_err(Into::into)
    }

    /// Decode a base payload and the log sequence it covers.
    pub(crate) fn decode_base_payload(
        payload: &[u8],
    ) -> Result<(u64, ProcessSnapshot), ProcessDocumentError> {
        SnapshotCodecV4::decode_base(payload).map_err(Into::into)
    }

    pub(crate) fn encode_log(
        snapshot: &ProcessSnapshot,
        sequence: u64,
        changes: &LogChanges,
    ) -> Result<Vec<u8>, ProcessDocumentError> {
        let entry = LogPayload::collect(snapshot, sequence, changes)?;
        let payload = SnapshotCodecV4::encode_log(&entry)?;
        ProcessEnvelope::seal_v2(payload).map_err(Into::into)
    }

    #[cfg(test)]
    pub(crate) fn decode_log(bytes: &[u8]) -> Result<LogPayload, ProcessDocumentError> {
        let envelope = ProcessEnvelope::decode(bytes)?;
        if envelope.schema_version != crate::scheduler::durable::envelope::AGGREGATE_SCHEMA_VERSION
        {
            return Err(EnvelopeError::UnsupportedSchema(envelope.schema_version).into());
        }
        Self::decode_log_payload(&envelope.payload)
    }

    pub(crate) fn decode_log_payload(payload: &[u8]) -> Result<LogPayload, ProcessDocumentError> {
        SnapshotCodecV4::decode_log(payload).map_err(Into::into)
    }

    /// Rebuild the aggregate from the base snapshot and the ordered log
    /// entries in `[head.base_seq, head.next_seq)`.
    pub(crate) fn assemble(
        base: ProcessSnapshot,
        head: &HeadPayload,
        entries: &[LogPayload],
    ) -> Result<ProcessSnapshot, ProcessDocumentError> {
        if base.process_id != head.process_id {
            return Err(SnapshotCodecError::InvalidLog(format!(
                "base process {:?} does not match head {:?}",
                base.process_id, head.process_id
            ))
            .into());
        }
        let mut snapshot = base;
        snapshot.version = head.version.clone();
        for entry in entries {
            if entry.process_id != head.process_id {
                return Err(SnapshotCodecError::InvalidLog(format!(
                    "log entry {:?} belongs to {:?}, not {:?}",
                    entry.sequence, entry.process_id, head.process_id
                ))
                .into());
            }
            entry.apply_to(&mut snapshot);
        }
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

    fn all_changes(snapshot: &ProcessSnapshot) -> LogChanges {
        LogChanges {
            tasks: snapshot.tasks.keys().cloned().collect(),
            variables: snapshot.variables.keys().cloned().collect(),
            operations: snapshot.operations.keys().cloned().collect(),
            outbox: snapshot.outbox.keys().cloned().collect(),
        }
    }

    /// The L8 family rebuilds exactly the aggregate the legacy document held:
    /// head + base + the records one commit changed assemble to a value-equal
    /// snapshot, and the log entry decodes through the envelope boundary.
    #[test]
    fn family_round_trip_rebuilds_the_aggregate() {
        let source = snapshot();
        let changes = all_changes(&source);
        let head = ProcessLogCodec::encode_head(&HeadPayload {
            process_id: source.process_id.clone(),
            version: source.version.clone(),
            epoch: 7,
            base_seq: 0,
            next_seq: 1,
        })
        .unwrap();
        let base = ProcessLogCodec::encode_base(&source, 0).unwrap();
        let log = ProcessLogCodec::encode_log(&source, 0, &changes).unwrap();

        let head = ProcessLogCodec::decode_head(&head).unwrap();
        let base_envelope = ProcessEnvelope::decode(&base).unwrap();
        let (base_seq, base) =
            ProcessLogCodec::decode_base_payload(&base_envelope.payload).unwrap();
        assert_eq!(base_seq, 0);
        let entry = ProcessLogCodec::decode_log(&log).unwrap();
        assert_eq!(entry.sequence, 0);
        let assembled = ProcessLogCodec::assemble(base, &head, &[entry]).unwrap();
        assert_eq!(assembled, source);
    }

    /// Each reader only accepts its own schema; a document sealed for the
    /// other family is an unsupported version, not a payload failure.
    #[test]
    fn readers_reject_the_other_schema_family() {
        let l8 = ProcessEnvelope::seal_v2(b"anything".to_vec()).unwrap();
        assert_eq!(
            ProcessDocumentCodec::decode(&l8),
            Err(ProcessDocumentError::Envelope(
                EnvelopeError::UnsupportedSchema(
                    crate::scheduler::durable::envelope::AGGREGATE_SCHEMA_VERSION
                )
            ))
        );

        let legacy = ProcessEnvelope::seal(b"anything".to_vec()).unwrap();
        let expected = EnvelopeError::UnsupportedSchema(
            crate::scheduler::durable::envelope::LEGACY_AGGREGATE_SCHEMA_VERSION,
        );
        assert_eq!(
            ProcessLogCodec::decode_head(&legacy),
            Err(ProcessDocumentError::Envelope(expected.clone()))
        );
        assert_eq!(
            ProcessLogCodec::decode_log(&legacy),
            Err(ProcessDocumentError::Envelope(expected))
        );
    }

    /// Assembly refuses a base or a log entry from another aggregate instead
    /// of mixing two processes into one snapshot.
    #[test]
    fn assembly_rejects_documents_from_another_process() {
        let source = snapshot();
        let head = HeadPayload {
            process_id: source.process_id.clone(),
            version: source.version.clone(),
            epoch: 1,
            base_seq: 0,
            next_seq: 1,
        };
        let mut foreign = source.clone();
        foreign.process_id = "foreign".into();
        let error = ProcessLogCodec::assemble(foreign, &head, &[]).unwrap_err();
        assert!(error.to_string().contains("does not match"));

        let entry = LogPayload {
            process_id: "foreign".into(),
            sequence: 0,
            tasks: Vec::new(),
            variables: Vec::new(),
            operations: Vec::new(),
            outbox: Vec::new(),
        };
        let error = ProcessLogCodec::assemble(source.clone(), &head, &[entry]).unwrap_err();
        assert!(error.to_string().contains("belongs to"));
    }

    #[test]
    fn document_error_display_names_the_document() {
        let error = ProcessDocumentError::Snapshot(SnapshotCodecError::InvalidMagic);
        assert!(error.to_string().contains("aggregate document error"));
        assert_eq!(error, SnapshotCodecError::InvalidMagic.into());
        assert_eq!(
            ProcessDocumentError::Envelope(EnvelopeError::TruncatedHeader),
            EnvelopeError::TruncatedHeader.into()
        );
    }

    /// The L8 golden. The V3 *payload* golden above did not move: the base and
    /// every log record keep the V3 record layout. What L8 changed is the
    /// document family — a head payload, a base snapshot and a per-commit log
    /// entry — so those three framings are frozen here. Freezing them is
    /// deliberate: a format change must update this test and the CHANGELOG
    /// together, not drift silently.
    #[test]
    fn family_wire_format_matches_golden_fingerprint() {
        use crate::scheduler::core::commit::aggregate::sha256_hex;

        let source = snapshot();
        let changes = all_changes(&source);
        let head = ProcessLogCodec::encode_head(&HeadPayload {
            process_id: source.process_id.clone(),
            version: source.version.clone(),
            epoch: 7,
            base_seq: 0,
            next_seq: 1,
        })
        .unwrap();
        let base = ProcessLogCodec::encode_base(&source, 0).unwrap();
        let log = ProcessLogCodec::encode_log(&source, 0, &changes).unwrap();
        assert_eq!(head.len(), 149);
        assert_eq!(
            sha256_hex(&head),
            "0b0c06f9f9f295fd9a97cd9fb97bc251c76c2e680122cc7814f25bb20f471bf6"
        );
        assert_eq!(base.len(), 431);
        assert_eq!(
            sha256_hex(&base),
            "0696f88f8821a655737fe3a6dedd0c2a2a6bbb5349c29225036f96595ccbb649"
        );
        assert_eq!(log.len(), 395);
        assert_eq!(
            sha256_hex(&log),
            "1531ee230b05ca2a4517b9089250ab31e2de2ec0d105615978817d055d3e7157"
        );
    }
}
