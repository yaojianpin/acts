use crate::scheduler::core::{
    commit::process::{CommitError, ProcessSnapshot, TaskSnapshot},
    model::{
        model_graph::NodeId,
        operation::{
            ClaimToken, CommitToken, OperationId, OperationKind, OperationRecord, OperationState,
        },
        task_graph::{TaskId, TaskState},
        transition_plan::ProcessVersion,
    },
};

use std::collections::{BTreeMap, HashMap};
use std::fmt;

const PAYLOAD_MAGIC: &[u8; 8] = b"ACTSPV2\0";
const MIN_EXECUTION_BYTES: usize = 4 + 8 + 4 + 1 + 4 + 1 + 8;
const MIN_EXECUTION_ID_BYTES: usize = 4 + 8;
const MIN_VARIABLE_BYTES: usize = MIN_EXECUTION_ID_BYTES + 4 + 4;
const MIN_OPERATION_BYTES: usize = 4 + 4 + 8 + 8 + 1 + 1 + 8;
const MIN_OUTBOX_FIELD_BYTES: usize = 4;

pub(crate) struct SnapshotCodecV2;

impl SnapshotCodecV2 {
    pub(crate) fn encode(snapshot: &ProcessSnapshot) -> Result<Vec<u8>, SnapshotCodecError> {
        snapshot
            .validate()
            .map_err(SnapshotCodecError::InvalidProcess)?;
        let mut writer = Writer::default();
        writer.raw(PAYLOAD_MAGIC);
        writer.string(&snapshot.process_id)?;
        writer.u64(snapshot.version.graph);
        writer.u64(snapshot.version.process);
        writer.u64(snapshot.version.operations);

        let mut tasks = snapshot.tasks.iter().collect::<Vec<_>>();
        tasks.sort_by_key(|(id, _)| *id);
        writer.count(tasks.len())?;
        for (id, value) in tasks {
            writer.task_id(id)?;
            writer.string(value.model_node.as_str())?;
            writer.optional_task(value.parent.as_ref())?;
            let mut predecessors = value.predecessors.clone();
            predecessors.sort();
            writer.count(predecessors.len())?;
            for predecessor in &predecessors {
                writer.task_id(predecessor)?;
            }
            writer.u8(task_state_tag(value.state));
            writer.u64(value.revision);
        }

        writer.count(snapshot.variables.len())?;
        for ((scope, name), value) in &snapshot.variables {
            writer.task_id(scope)?;
            writer.string(name)?;
            writer.string(value)?;
        }

        let mut operations = snapshot.operations.values().collect::<Vec<_>>();
        operations.sort_by(|left, right| operation_id_order(&left.id, &right.id));
        writer.count(operations.len())?;
        for operation in operations {
            writer.operation(operation)?;
        }

        let mut outbox = snapshot.outbox.values().collect::<Vec<_>>();
        outbox.sort_by(|left, right| left.id.cmp(&right.id));
        writer.count(outbox.len())?;
        for record in outbox {
            let bytes = crate::scheduler::core::outbox::outbox_codec::OutboxCodec::encode(record)
                .map_err(|error| SnapshotCodecError::InvalidOutbox(error.to_string()))?;
            writer.bytes(&bytes)?;
        }
        Ok(writer.bytes)
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<ProcessSnapshot, SnapshotCodecError> {
        let mut reader = Reader::new(bytes);
        if reader.raw(PAYLOAD_MAGIC.len())? != PAYLOAD_MAGIC {
            return Err(SnapshotCodecError::InvalidMagic);
        }
        let process_id = reader.string()?;
        let version = ProcessVersion {
            graph: reader.u64()?,
            process: reader.u64()?,
            operations: reader.u64()?,
        };

        let task_count = reader.count_bounded(MIN_EXECUTION_BYTES)?;
        let mut tasks = HashMap::with_capacity(task_count);
        for _ in 0..task_count {
            let id = reader.task_id()?;
            let model_node =
                NodeId::parse(reader.string()?).map_err(|_| SnapshotCodecError::InvalidNodeId)?;
            let parent = reader.optional_task()?;
            let predecessor_count = reader.count_bounded(MIN_EXECUTION_ID_BYTES)?;
            let mut predecessors = Vec::with_capacity(predecessor_count);
            for _ in 0..predecessor_count {
                predecessors.push(reader.task_id()?);
            }
            let state = decode_task_state(reader.u8()?)?;
            let revision = reader.u64()?;
            if tasks
                .insert(
                    id.clone(),
                    TaskSnapshot {
                        model_node,
                        parent,
                        predecessors,
                        state,
                        revision,
                    },
                )
                .is_some()
            {
                return Err(SnapshotCodecError::DuplicateTask(id));
            }
        }

        let variable_count = reader.count_bounded(MIN_VARIABLE_BYTES)?;
        let mut variables = BTreeMap::new();
        for _ in 0..variable_count {
            let scope = reader.task_id()?;
            let name = reader.string()?;
            let value = reader.string()?;
            if variables
                .insert((scope.clone(), name.clone()), value)
                .is_some()
            {
                return Err(SnapshotCodecError::DuplicateVariable(scope, name));
            }
        }

        let operation_count = reader.count_bounded(MIN_OPERATION_BYTES)?;
        let mut operations = HashMap::with_capacity(operation_count);
        for _ in 0..operation_count {
            let operation = reader.operation()?;
            if operations
                .insert(operation.id.clone(), operation.clone())
                .is_some()
            {
                return Err(SnapshotCodecError::DuplicateOperation(operation.id));
            }
        }

        let outbox_count = reader.count_bounded(MIN_OUTBOX_FIELD_BYTES)?;
        let mut outbox = HashMap::with_capacity(outbox_count);
        for _ in 0..outbox_count {
            let record =
                crate::scheduler::core::outbox::outbox_codec::OutboxCodec::decode(reader.bytes()?)
                    .map_err(|error| SnapshotCodecError::InvalidOutbox(error.to_string()))?;
            if outbox.insert(record.id.clone(), record.clone()).is_some() {
                return Err(SnapshotCodecError::DuplicateOutboxKey(record.id.key));
            }
        }
        if !reader.is_finished() {
            return Err(SnapshotCodecError::TrailingBytes);
        }
        let snapshot = ProcessSnapshot {
            process_id,
            version,
            tasks,
            variables,
            operations,
            outbox,
        };
        snapshot
            .validate()
            .map_err(SnapshotCodecError::InvalidProcess)?;
        Ok(snapshot)
    }
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
    fn u32(&mut self, value: u32) {
        self.raw(&value.to_be_bytes());
    }
    fn u64(&mut self, value: u64) {
        self.raw(&value.to_be_bytes());
    }
    fn i64(&mut self, value: i64) {
        self.raw(&value.to_be_bytes());
    }
    fn count(&mut self, value: usize) -> Result<(), SnapshotCodecError> {
        self.u32(u32::try_from(value).map_err(|_| SnapshotCodecError::CollectionTooLarge)?);
        Ok(())
    }
    fn bytes(&mut self, value: &[u8]) -> Result<(), SnapshotCodecError> {
        self.count(value.len())?;
        self.raw(value);
        Ok(())
    }
    fn string(&mut self, value: &str) -> Result<(), SnapshotCodecError> {
        self.count(value.len())?;
        self.raw(value.as_bytes());
        Ok(())
    }
    fn task_id(&mut self, id: &TaskId) -> Result<(), SnapshotCodecError> {
        self.string(&id.node)?;
        self.u64(id.generation);
        Ok(())
    }
    fn optional_task(&mut self, id: Option<&TaskId>) -> Result<(), SnapshotCodecError> {
        match id {
            Some(id) => {
                self.u8(1);
                self.task_id(id)?;
            }
            None => self.u8(0),
        }
        Ok(())
    }
    fn operation_id(&mut self, id: &OperationId) -> Result<(), SnapshotCodecError> {
        self.string(&id.process_id)?;
        self.string(&id.node)?;
        self.u64(id.generation);
        self.u64(id.sequence);
        Ok(())
    }
    fn claim(&mut self, claim: &ClaimToken) -> Result<(), SnapshotCodecError> {
        self.string(&claim.owner)?;
        self.u64(claim.fence);
        self.i64(claim.lease_until_ms);
        self.u64(claim.heartbeat_revision);
        Ok(())
    }
    fn commit(&mut self, commit: &CommitToken) -> Result<(), SnapshotCodecError> {
        self.u64(commit.process_version);
        self.string(&commit.digest)
    }
    fn operation(&mut self, operation: &OperationRecord) -> Result<(), SnapshotCodecError> {
        self.operation_id(&operation.id)?;
        self.u8(operation_kind_tag(operation.kind));
        self.u64(operation.revision);
        match &operation.state {
            OperationState::Pending => self.u8(0),
            OperationState::Claimed(claim) => {
                self.u8(1);
                self.claim(claim)?;
            }
            OperationState::CommitPrepared(claim) => {
                self.u8(2);
                self.claim(claim)?;
            }
            OperationState::Committed(commit) => {
                self.u8(3);
                self.commit(commit)?;
            }
            OperationState::RetryScheduled { attempt } => {
                self.u8(4);
                self.u32(*attempt);
            }
            OperationState::EffectUnknown { attempt } => {
                self.u8(5);
                self.u32(*attempt);
            }
            OperationState::Quarantined { reason } => {
                self.u8(6);
                self.string(reason)?;
            }
            OperationState::Superseded { by_generation } => {
                self.u8(7);
                self.u64(*by_generation);
            }
            OperationState::Completed(commit) => {
                self.u8(8);
                self.commit(commit)?;
            }
        }
        Ok(())
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }
    fn raw(&mut self, length: usize) -> Result<&'a [u8], SnapshotCodecError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(SnapshotCodecError::Truncated)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(SnapshotCodecError::Truncated)?;
        self.offset = end;
        Ok(value)
    }
    fn u8(&mut self) -> Result<u8, SnapshotCodecError> {
        Ok(self.raw(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, SnapshotCodecError> {
        Ok(u32::from_be_bytes(
            self.raw(4)?
                .try_into()
                .map_err(|_| SnapshotCodecError::Truncated)?,
        ))
    }
    fn u64(&mut self) -> Result<u64, SnapshotCodecError> {
        Ok(u64::from_be_bytes(
            self.raw(8)?
                .try_into()
                .map_err(|_| SnapshotCodecError::Truncated)?,
        ))
    }
    fn i64(&mut self) -> Result<i64, SnapshotCodecError> {
        Ok(i64::from_be_bytes(
            self.raw(8)?
                .try_into()
                .map_err(|_| SnapshotCodecError::Truncated)?,
        ))
    }
    fn remaining(&self) -> usize {
        self.bytes.len() - self.offset
    }
    fn count(&mut self) -> Result<usize, SnapshotCodecError> {
        usize::try_from(self.u32()?).map_err(|_| SnapshotCodecError::CollectionTooLarge)
    }
    fn count_bounded(&mut self, minimum_item_bytes: usize) -> Result<usize, SnapshotCodecError> {
        let count = self.count()?;
        if minimum_item_bytes == 0 || count > self.remaining() / minimum_item_bytes {
            return Err(SnapshotCodecError::Truncated);
        }
        Ok(count)
    }
    fn bytes(&mut self) -> Result<&'a [u8], SnapshotCodecError> {
        let length = self.count()?;
        self.raw(length)
    }
    fn string(&mut self) -> Result<String, SnapshotCodecError> {
        let length = self.count()?;
        String::from_utf8(self.raw(length)?.to_vec()).map_err(|_| SnapshotCodecError::InvalidUtf8)
    }
    fn task_id(&mut self) -> Result<TaskId, SnapshotCodecError> {
        TaskId::new(self.string()?, self.u64()?).map_err(|_| SnapshotCodecError::InvalidTaskId)
    }
    fn optional_task(&mut self) -> Result<Option<TaskId>, SnapshotCodecError> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.task_id()?)),
            value => Err(SnapshotCodecError::InvalidTag(value)),
        }
    }
    fn operation_id(&mut self) -> Result<OperationId, SnapshotCodecError> {
        Ok(OperationId {
            process_id: self.string()?,
            node: self.string()?,
            generation: self.u64()?,
            sequence: self.u64()?,
        })
    }
    fn claim(&mut self) -> Result<ClaimToken, SnapshotCodecError> {
        Ok(ClaimToken {
            owner: self.string()?,
            fence: self.u64()?,
            lease_until_ms: self.i64()?,
            heartbeat_revision: self.u64()?,
        })
    }
    fn commit(&mut self) -> Result<CommitToken, SnapshotCodecError> {
        Ok(CommitToken {
            process_version: self.u64()?,
            digest: self.string()?,
        })
    }
    fn operation(&mut self) -> Result<OperationRecord, SnapshotCodecError> {
        let id = self.operation_id()?;
        let kind = decode_operation_kind(self.u8()?)?;
        let revision = self.u64()?;
        let state = match self.u8()? {
            0 => OperationState::Pending,
            1 => OperationState::Claimed(self.claim()?),
            2 => OperationState::CommitPrepared(self.claim()?),
            3 => OperationState::Committed(self.commit()?),
            4 => OperationState::RetryScheduled {
                attempt: self.u32()?,
            },
            5 => OperationState::EffectUnknown {
                attempt: self.u32()?,
            },
            6 => OperationState::Quarantined {
                reason: self.string()?,
            },
            7 => OperationState::Superseded {
                by_generation: self.u64()?,
            },
            8 => OperationState::Completed(self.commit()?),
            value => return Err(SnapshotCodecError::InvalidTag(value)),
        };
        Ok(OperationRecord {
            id,
            kind,
            state,
            revision,
        })
    }
    fn is_finished(&self) -> bool {
        self.offset == self.bytes.len()
    }
}

fn task_state_tag(state: TaskState) -> u8 {
    use TaskState::*;
    match state {
        Created => 0,
        Ready => 1,
        Running => 2,
        Waiting => 3,
        Completed => 4,
        Submitted => 5,
        Backed => 6,
        Cancelled => 7,
        Failed => 8,
        Aborted => 9,
        Skipped => 10,
        Removed => 11,
    }
}
fn decode_task_state(tag: u8) -> Result<TaskState, SnapshotCodecError> {
    use TaskState::*;
    match tag {
        0 => Ok(Created),
        1 => Ok(Ready),
        2 => Ok(Running),
        3 => Ok(Waiting),
        4 => Ok(Completed),
        5 => Ok(Submitted),
        6 => Ok(Backed),
        7 => Ok(Cancelled),
        8 => Ok(Failed),
        9 => Ok(Aborted),
        10 => Ok(Skipped),
        11 => Ok(Removed),
        value => Err(SnapshotCodecError::InvalidTag(value)),
    }
}
fn operation_kind_tag(kind: OperationKind) -> u8 {
    use OperationKind::*;
    match kind {
        ExecuteTask => 0,
        PropagateTask => 1,
        ApplyAction => 2,
        DeliverMessage => 3,
        FireTrigger => 4,
        RunPostCommit => 5,
        SkipTask => 6,
        // Appended, never reshuffled: stored snapshots pin these tags.
        AbortTask => 7,
        DispatchTimeout => 8,
        ReenterTask => 9,
    }
}
fn decode_operation_kind(tag: u8) -> Result<OperationKind, SnapshotCodecError> {
    use OperationKind::*;
    match tag {
        0 => Ok(ExecuteTask),
        1 => Ok(PropagateTask),
        2 => Ok(ApplyAction),
        3 => Ok(DeliverMessage),
        4 => Ok(FireTrigger),
        5 => Ok(RunPostCommit),
        6 => Ok(SkipTask),
        7 => Ok(AbortTask),
        8 => Ok(DispatchTimeout),
        9 => Ok(ReenterTask),
        value => Err(SnapshotCodecError::InvalidTag(value)),
    }
}
fn operation_id_order(left: &OperationId, right: &OperationId) -> std::cmp::Ordering {
    left.process_id
        .cmp(&right.process_id)
        .then_with(|| left.node.cmp(&right.node))
        .then_with(|| left.generation.cmp(&right.generation))
        .then_with(|| left.sequence.cmp(&right.sequence))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SnapshotCodecError {
    InvalidMagic,
    Truncated,
    TrailingBytes,
    InvalidUtf8,
    CollectionTooLarge,
    InvalidTag(u8),
    InvalidTaskId,
    InvalidNodeId,
    DuplicateTask(TaskId),
    DuplicateVariable(TaskId, String),
    DuplicateOperation(OperationId),
    DuplicateOutboxKey(String),
    InvalidOutbox(String),
    InvalidProcess(CommitError),
}
impl fmt::Display for SnapshotCodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "snapshot codec error: {self:?}")
    }
}
impl std::error::Error for SnapshotCodecError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded_u32(bytes: &[u8], offset: usize) -> u32 {
        let field = bytes
            .get(offset..offset + 4)
            .expect("test fixture contains the requested u32 field");
        u32::from_be_bytes(field.try_into().expect("test fixture u32 has four bytes"))
    }
    use crate::scheduler::core::commit::process::MemoryProcessRepository;
    use crate::scheduler::core::commit::process::{
        MemoryProcessRepositoryAccess, ProcessRepository,
    };

    fn snapshot() -> ProcessSnapshot {
        let root = TaskId::new("root", 0).unwrap();
        let child = TaskId::new("child", 1).unwrap();
        let operation_id = OperationId {
            process_id: "p".into(),
            node: "child".into(),
            generation: 1,
            sequence: 4,
        };
        let claim = ClaimToken {
            owner: "runtime".into(),
            fence: 9,
            lease_until_ms: 1000,
            heartbeat_revision: 3,
        };
        ProcessSnapshot {
            process_id: "p".into(),
            version: ProcessVersion {
                graph: 2,
                process: 8,
                operations: 9,
            },
            tasks: HashMap::from([
                (
                    root.clone(),
                    TaskSnapshot {
                        model_node: NodeId::parse("model-root").unwrap(),
                        parent: None,
                        predecessors: vec![],
                        state: TaskState::Waiting,
                        revision: 2,
                    },
                ),
                (
                    child.clone(),
                    TaskSnapshot {
                        model_node: NodeId::parse("model-child").unwrap(),
                        parent: Some(root.clone()),
                        predecessors: vec![root],
                        state: TaskState::Running,
                        revision: 4,
                    },
                ),
            ]),
            variables: BTreeMap::from([((child, "answer".into()), "a:b|c".into())]),
            operations: HashMap::from([(
                operation_id.clone(),
                OperationRecord {
                    id: operation_id,
                    kind: OperationKind::ExecuteTask,
                    state: OperationState::Claimed(claim),
                    revision: 7,
                },
            )]),
            outbox: HashMap::new(),
        }
    }

    #[test]
    fn round_trip_preserves_complete_domain_snapshot() {
        let source = snapshot();
        let encoded = SnapshotCodecV2::encode(&source).unwrap();
        assert_eq!(SnapshotCodecV2::decode(&encoded).unwrap(), source);
    }

    #[test]
    fn encoding_is_stable_across_map_insertion_order() {
        let source = snapshot();
        let mut reordered = ProcessSnapshot::empty("p").unwrap();
        reordered.version = source.version.clone();
        let mut tasks = source.tasks.iter().collect::<Vec<_>>();
        tasks.sort_by(|(left, _), (right, _)| right.cmp(left));
        for (id, value) in tasks {
            reordered.tasks.insert(id.clone(), value.clone());
        }
        let mut operations = source.operations.iter().collect::<Vec<_>>();
        operations.sort_by(|(left, _), (right, _)| operation_id_order(right, left));
        for (id, value) in operations {
            reordered.operations.insert(id.clone(), value.clone());
        }
        reordered.variables = source.variables.clone();
        reordered.outbox = source.outbox.clone();
        assert_eq!(
            SnapshotCodecV2::encode(&source).unwrap(),
            SnapshotCodecV2::encode(&reordered).unwrap()
        );
    }

    #[test]
    fn truncated_and_trailing_payloads_are_rejected() {
        let encoded = SnapshotCodecV2::encode(&snapshot()).unwrap();
        for length in 0..encoded.len() {
            assert!(SnapshotCodecV2::decode(&encoded[..length]).is_err());
        }
        let mut trailing = encoded;
        trailing.push(0);
        assert_eq!(
            SnapshotCodecV2::decode(&trailing),
            Err(SnapshotCodecError::TrailingBytes)
        );
    }

    #[test]
    fn decoded_snapshot_passes_repository_validation_boundary() {
        let decoded =
            SnapshotCodecV2::decode(&SnapshotCodecV2::encode(&snapshot()).unwrap()).unwrap();
        let mut repository = MemoryProcessRepository::default();
        repository.insert_for_test(decoded).unwrap();
        assert!(repository.load("p").unwrap().is_some());
    }
    #[test]
    fn impossible_task_count_is_rejected_before_allocation() {
        let mut encoded = SnapshotCodecV2::encode(&snapshot()).unwrap();
        let process_length = usize::try_from(encoded_u32(&encoded, 8)).unwrap();
        let task_count_offset = 12 + process_length + 24;
        encoded[task_count_offset..task_count_offset + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(
            SnapshotCodecV2::decode(&encoded),
            Err(SnapshotCodecError::Truncated)
        );
    }
    #[test]
    fn snapshot_wire_format_matches_golden_fingerprint() {
        let encoded = SnapshotCodecV2::encode(&snapshot()).unwrap();
        assert_eq!(encoded.len(), 286);
        assert_eq!(
            crate::scheduler::core::commit::process::sha256_hex(&encoded),
            "26dc1279e7f4eddd825ee0c655877f26236ad360ba3ae62adcfac54168b7e693"
        );
    }
}
