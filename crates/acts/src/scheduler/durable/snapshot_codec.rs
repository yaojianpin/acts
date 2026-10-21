use crate::scheduler::core::{
    commit::aggregate::{CommitError, ProcessSnapshot, TaskSnapshot},
    model::{
        model_graph::NodeId,
        operation::{
            ClaimToken, CommitToken, OperationId, OperationKind, OperationRecord, OperationState,
        },
        task_graph::{TaskId, TaskState},
        transition_plan::{ProcessVersion, TransitionPlan},
    },
    outbox::records::OutboxId,
};

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;

const PAYLOAD_MAGIC: &[u8; 8] = b"ACTSPV2\0";
const PAYLOAD_MAGIC_V3: &[u8; 8] = b"ACTSPV3\0";
/// L8 head payload: identity, aggregate version and the log window.
pub(crate) const PAYLOAD_MAGIC_V4_HEAD: &[u8; 8] = b"ACTSPH4\0";
/// L8 base payload: the log sequence the base snapshot covers, then a V3
/// snapshot. Binding the two lets a reader reject a base paired with a head
/// from another version.
pub(crate) const PAYLOAD_MAGIC_V4_BASE: &[u8; 8] = b"ACTSPB4\0";
/// L8 log payload: the records one commit changed.
pub(crate) const PAYLOAD_MAGIC_V4_LOG: &[u8; 8] = b"ACTSPL4\0";
const MIN_EXECUTION_BYTES: usize = 4 + 8 + 4 + 1 + 4 + 1 + 8;
const MIN_EXECUTION_ID_BYTES: usize = 4 + 8;
const MIN_VARIABLE_BYTES: usize = MIN_EXECUTION_ID_BYTES + 4 + 4;
const MIN_OPERATION_BYTES: usize = 4 + 4 + 8 + 8 + 1 + 1 + 8;
const MIN_OUTBOX_FIELD_BYTES: usize = 4;

// V3 (C5, design §3.1): every identity carries its wire — the rendered
// instance id `sha256(pid, node, gen)[..32]` — next to the node string and
// the generation, so a document states the external id its instances were
// written under instead of leaving it to a derivation. One extra length
// prefix per id-bearing minimum.
const MIN_EXECUTION_BYTES_V3: usize = MIN_EXECUTION_BYTES + 4;
const MIN_EXECUTION_ID_BYTES_V3: usize = 4 + 4 + 8;
const MIN_VARIABLE_BYTES_V3: usize = MIN_EXECUTION_ID_BYTES_V3 + 4 + 4;
const MIN_OPERATION_BYTES_V3: usize = MIN_OPERATION_BYTES + 4;

pub(crate) struct SnapshotCodecV2;

impl SnapshotCodecV2 {
    /// The V2 writer. Production writes V3 (the versioned identity); this
    /// writer survives only as the test fixture builder for the upcast — V2
    /// documents an earlier kernel wrote must keep loading through
    /// [`SnapshotCodecV2::decode`].
    #[cfg(test)]
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

/// The C5 document format: V2's layout plus the identity's wire on every
/// `TaskId` and `OperationId.task` (design §3.1). The stored wire is the
/// instance id as rendered at write time; the `TaskId` itself still rebuilds
/// from the node string and the generation, and the derivation is pure, so
/// the two agree by construction — decode never recomputes the digest (the
/// read path stays one string parse per id).
pub(crate) struct SnapshotCodecV3;

impl SnapshotCodecV3 {
    pub(crate) fn encode(snapshot: &ProcessSnapshot) -> Result<Vec<u8>, SnapshotCodecError> {
        snapshot
            .validate()
            .map_err(SnapshotCodecError::InvalidProcess)?;
        let mut writer = Writer::default();
        writer.raw(PAYLOAD_MAGIC_V3);
        writer.string(&snapshot.process_id)?;
        writer.u64(snapshot.version.graph);
        writer.u64(snapshot.version.process);
        writer.u64(snapshot.version.operations);

        let mut tasks = snapshot.tasks.iter().collect::<Vec<_>>();
        tasks.sort_by_key(|(id, _)| *id);
        writer.count(tasks.len())?;
        for (id, value) in tasks {
            writer.task_record(id, value, &snapshot.process_id)?;
        }

        writer.count(snapshot.variables.len())?;
        for ((scope, name), value) in &snapshot.variables {
            writer.variable_record(scope, name, value, &snapshot.process_id)?;
        }

        let mut operations = snapshot.operations.values().collect::<Vec<_>>();
        operations.sort_by(|left, right| operation_id_order(&left.id, &right.id));
        writer.count(operations.len())?;
        for operation in operations {
            writer.wire_operation(operation)?;
        }

        let mut outbox = snapshot.outbox.values().collect::<Vec<_>>();
        outbox.sort_by(|left, right| left.id.cmp(&right.id));
        writer.count(outbox.len())?;
        for record in outbox {
            writer.outbox_record(record)?;
        }
        Ok(writer.bytes)
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<ProcessSnapshot, SnapshotCodecError> {
        let mut reader = Reader::new(bytes);
        if reader.raw(PAYLOAD_MAGIC_V3.len())? != PAYLOAD_MAGIC_V3 {
            return Err(SnapshotCodecError::InvalidMagic);
        }
        let process_id = reader.string()?;
        let version = ProcessVersion {
            graph: reader.u64()?,
            process: reader.u64()?,
            operations: reader.u64()?,
        };

        let task_count = reader.count_bounded(MIN_EXECUTION_BYTES_V3)?;
        let mut tasks = HashMap::with_capacity(task_count);
        for _ in 0..task_count {
            let (id, value) = reader.task_record()?;
            if tasks.insert(id.clone(), value).is_some() {
                return Err(SnapshotCodecError::DuplicateTask(id));
            }
        }

        let variable_count = reader.count_bounded(MIN_VARIABLE_BYTES_V3)?;
        let mut variables = BTreeMap::new();
        for _ in 0..variable_count {
            let ((scope, name), value) = reader.variable_record()?;
            if variables
                .insert((scope.clone(), name.clone()), value)
                .is_some()
            {
                return Err(SnapshotCodecError::DuplicateVariable(scope, name));
            }
        }

        let operation_count = reader.count_bounded(MIN_OPERATION_BYTES_V3)?;
        let mut operations = HashMap::with_capacity(operation_count);
        for _ in 0..operation_count {
            let operation = reader.wire_operation()?;
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
            let record = reader.outbox_record()?;
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

/// The records one commit changed, relative to the aggregate the commit
/// loaded. A log entry carries only these; every other record stays in the
/// base snapshot and is not re-encoded.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct LogChanges {
    pub(crate) tasks: BTreeSet<TaskId>,
    pub(crate) variables: BTreeSet<(TaskId, String)>,
    pub(crate) operations: BTreeSet<OperationId>,
    pub(crate) outbox: BTreeSet<OutboxId>,
}

impl LogChanges {
    /// The changes `advance_transition` applies. This is the complete set:
    /// the advance inserts creates/operations/outbox records, bumps state
    /// changes and variable writes, and closes the source operation.
    pub(crate) fn from_transition(plan: &TransitionPlan) -> Self {
        let mut changes = Self::default();
        for create in &plan.task_creates {
            changes.tasks.insert(create.task.clone());
        }
        for change in &plan.state_changes {
            changes.tasks.insert(change.task.clone());
        }
        for write in &plan.variable_writes {
            changes
                .variables
                .insert((write.scope.clone(), write.name.clone()));
        }
        for intent in &plan.operations {
            changes.operations.insert(intent.id.clone());
        }
        if let Some(source) = &plan.source_operation {
            changes.operations.insert(source.id.clone());
        }
        for item in &plan.outbox {
            changes.outbox.insert(OutboxId {
                process_id: plan.process_id.clone(),
                key: item.key.clone(),
            });
        }
        changes
    }

    pub(crate) fn operation(id: OperationId) -> Self {
        let mut changes = Self::default();
        changes.operations.insert(id);
        changes
    }

    pub(crate) fn outbox(ids: impl IntoIterator<Item = OutboxId>) -> Self {
        let mut changes = Self::default();
        changes.outbox.extend(ids);
        changes
    }
}

/// The L8 head: the aggregate version and the log window
/// `[base_seq, next_seq)`. `epoch` advances on every write so the head bytes
/// are a version guard even for a commit that leaves the domain version
/// unchanged (an outbox confirmation).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HeadPayload {
    pub(crate) process_id: String,
    pub(crate) version: ProcessVersion,
    pub(crate) epoch: u64,
    pub(crate) base_seq: u64,
    pub(crate) next_seq: u64,
}

/// One log entry: exactly the records one commit changed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LogPayload {
    pub(crate) process_id: String,
    pub(crate) sequence: u64,
    pub(crate) tasks: Vec<(TaskId, TaskSnapshot)>,
    pub(crate) variables: Vec<((TaskId, String), String)>,
    pub(crate) operations: Vec<OperationRecord>,
    pub(crate) outbox: Vec<crate::scheduler::core::outbox::records::OutboxRecord>,
}

impl LogPayload {
    pub(crate) fn collect(
        snapshot: &ProcessSnapshot,
        sequence: u64,
        changes: &LogChanges,
    ) -> Result<Self, SnapshotCodecError> {
        let mut tasks = Vec::with_capacity(changes.tasks.len());
        for id in &changes.tasks {
            let value = snapshot
                .tasks
                .get(id)
                .ok_or_else(|| SnapshotCodecError::ChangedRecordMissing(format!("task {id:?}")))?;
            tasks.push((id.clone(), value.clone()));
        }
        let mut variables = Vec::with_capacity(changes.variables.len());
        for key in &changes.variables {
            let value = snapshot.variables.get(key).ok_or_else(|| {
                SnapshotCodecError::ChangedRecordMissing(format!("variable {key:?}"))
            })?;
            variables.push((key.clone(), value.clone()));
        }
        let mut operations = Vec::with_capacity(changes.operations.len());
        for id in &changes.operations {
            let value = snapshot.operations.get(id).ok_or_else(|| {
                SnapshotCodecError::ChangedRecordMissing(format!("operation {id:?}"))
            })?;
            operations.push(value.clone());
        }
        let mut outbox = Vec::with_capacity(changes.outbox.len());
        for id in &changes.outbox {
            let value = snapshot.outbox.get(id).ok_or_else(|| {
                SnapshotCodecError::ChangedRecordMissing(format!("outbox record {id:?}"))
            })?;
            outbox.push(value.clone());
        }
        Ok(Self {
            process_id: snapshot.process_id.clone(),
            sequence,
            tasks,
            variables,
            operations,
            outbox,
        })
    }

    /// Apply this entry on top of a snapshot. Records are overwritten: a log
    /// carries state, not a mutation script, and no commit deletes a record
    /// (A11 reclaims the whole family instead).
    pub(crate) fn apply_to(&self, snapshot: &mut ProcessSnapshot) {
        for (id, value) in &self.tasks {
            snapshot.tasks.insert(id.clone(), value.clone());
        }
        for (key, value) in &self.variables {
            snapshot.variables.insert(key.clone(), value.clone());
        }
        for operation in &self.operations {
            snapshot
                .operations
                .insert(operation.id.clone(), operation.clone());
        }
        for record in &self.outbox {
            snapshot.outbox.insert(record.id.clone(), record.clone());
        }
    }
}

pub(crate) struct SnapshotCodecV4;

impl SnapshotCodecV4 {
    pub(crate) fn encode_head(head: &HeadPayload) -> Result<Vec<u8>, SnapshotCodecError> {
        if head.process_id.trim().is_empty() {
            return Err(SnapshotCodecError::InvalidLog("empty process id".into()));
        }
        if head.base_seq > head.next_seq {
            return Err(SnapshotCodecError::InvalidLog(
                "log window is inverted".into(),
            ));
        }
        let mut writer = Writer::default();
        writer.raw(PAYLOAD_MAGIC_V4_HEAD);
        writer.string(&head.process_id)?;
        writer.u64(head.version.graph);
        writer.u64(head.version.process);
        writer.u64(head.version.operations);
        writer.u64(head.epoch);
        writer.u64(head.base_seq);
        writer.u64(head.next_seq);
        Ok(writer.bytes)
    }

    pub(crate) fn decode_head(bytes: &[u8]) -> Result<HeadPayload, SnapshotCodecError> {
        let mut reader = Reader::new(bytes);
        if reader.raw(PAYLOAD_MAGIC_V4_HEAD.len())? != PAYLOAD_MAGIC_V4_HEAD {
            return Err(SnapshotCodecError::InvalidMagic);
        }
        let head = HeadPayload {
            process_id: reader.string()?,
            version: ProcessVersion {
                graph: reader.u64()?,
                process: reader.u64()?,
                operations: reader.u64()?,
            },
            epoch: reader.u64()?,
            base_seq: reader.u64()?,
            next_seq: reader.u64()?,
        };
        if !reader.is_finished() {
            return Err(SnapshotCodecError::TrailingBytes);
        }
        if head.process_id.trim().is_empty() {
            return Err(SnapshotCodecError::InvalidLog("empty process id".into()));
        }
        if head.base_seq > head.next_seq {
            return Err(SnapshotCodecError::InvalidLog(
                "log window is inverted".into(),
            ));
        }
        Ok(head)
    }

    pub(crate) fn encode_base(
        snapshot: &ProcessSnapshot,
        base_seq: u64,
    ) -> Result<Vec<u8>, SnapshotCodecError> {
        let payload = SnapshotCodecV3::encode(snapshot)?;
        let mut writer = Writer::default();
        writer.raw(PAYLOAD_MAGIC_V4_BASE);
        writer.u64(base_seq);
        writer.bytes(&payload)?;
        Ok(writer.bytes)
    }

    pub(crate) fn decode_base(bytes: &[u8]) -> Result<(u64, ProcessSnapshot), SnapshotCodecError> {
        let mut reader = Reader::new(bytes);
        if reader.raw(PAYLOAD_MAGIC_V4_BASE.len())? != PAYLOAD_MAGIC_V4_BASE {
            return Err(SnapshotCodecError::InvalidMagic);
        }
        let base_seq = reader.u64()?;
        let snapshot = SnapshotCodecV3::decode(reader.bytes()?)?;
        if !reader.is_finished() {
            return Err(SnapshotCodecError::TrailingBytes);
        }
        Ok((base_seq, snapshot))
    }

    pub(crate) fn encode_log(entry: &LogPayload) -> Result<Vec<u8>, SnapshotCodecError> {
        let mut writer = Writer::default();
        writer.raw(PAYLOAD_MAGIC_V4_LOG);
        writer.string(&entry.process_id)?;
        writer.u64(entry.sequence);
        writer.count(entry.tasks.len())?;
        for (id, value) in &entry.tasks {
            writer.task_record(id, value, &entry.process_id)?;
        }
        writer.count(entry.variables.len())?;
        for ((scope, name), value) in &entry.variables {
            writer.variable_record(scope, name, value, &entry.process_id)?;
        }
        writer.count(entry.operations.len())?;
        for operation in &entry.operations {
            writer.wire_operation(operation)?;
        }
        writer.count(entry.outbox.len())?;
        for record in &entry.outbox {
            writer.outbox_record(record)?;
        }
        Ok(writer.bytes)
    }

    pub(crate) fn decode_log(bytes: &[u8]) -> Result<LogPayload, SnapshotCodecError> {
        let mut reader = Reader::new(bytes);
        if reader.raw(PAYLOAD_MAGIC_V4_LOG.len())? != PAYLOAD_MAGIC_V4_LOG {
            return Err(SnapshotCodecError::InvalidMagic);
        }
        let process_id = reader.string()?;
        let sequence = reader.u64()?;

        let task_count = reader.count_bounded(MIN_EXECUTION_BYTES_V3)?;
        let mut tasks = Vec::with_capacity(task_count);
        let mut task_ids = std::collections::HashSet::with_capacity(task_count);
        for _ in 0..task_count {
            let (id, value) = reader.task_record()?;
            if !task_ids.insert(id.clone()) {
                return Err(SnapshotCodecError::DuplicateTask(id));
            }
            tasks.push((id, value));
        }

        let variable_count = reader.count_bounded(MIN_VARIABLE_BYTES_V3)?;
        let mut variables = Vec::with_capacity(variable_count);
        let mut variable_keys = std::collections::HashSet::with_capacity(variable_count);
        for _ in 0..variable_count {
            let record = reader.variable_record()?;
            if !variable_keys.insert(record.0.clone()) {
                return Err(SnapshotCodecError::DuplicateVariable(
                    record.0.0.clone(),
                    record.0.1.clone(),
                ));
            }
            variables.push(record);
        }

        let operation_count = reader.count_bounded(MIN_OPERATION_BYTES_V3)?;
        let mut operations = Vec::with_capacity(operation_count);
        let mut operation_ids = std::collections::HashSet::with_capacity(operation_count);
        for _ in 0..operation_count {
            let operation = reader.wire_operation()?;
            if !operation_ids.insert(operation.id.clone()) {
                return Err(SnapshotCodecError::DuplicateOperation(operation.id));
            }
            operations.push(operation);
        }

        let outbox_count = reader.count_bounded(MIN_OUTBOX_FIELD_BYTES)?;
        let mut outbox = Vec::with_capacity(outbox_count);
        let mut outbox_keys = std::collections::HashSet::with_capacity(outbox_count);
        for _ in 0..outbox_count {
            let record = reader.outbox_record()?;
            if !outbox_keys.insert(record.id.clone()) {
                return Err(SnapshotCodecError::DuplicateOutboxKey(record.id.key));
            }
            outbox.push(record);
        }
        if !reader.is_finished() {
            return Err(SnapshotCodecError::TrailingBytes);
        }
        Ok(LogPayload {
            process_id,
            sequence,
            tasks,
            variables,
            operations,
            outbox,
        })
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
    /// The V2 id shapes — the fixture writer's vocabulary only ([`SnapshotCodecV2::encode`]
    /// is `cfg(test)`); production writes the wire-shaped ids below.
    #[cfg(test)]
    fn task_id(&mut self, id: &TaskId) -> Result<(), SnapshotCodecError> {
        self.string(id.node.as_str())?;
        self.u64(id.generation);
        Ok(())
    }
    #[cfg(test)]
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
    #[cfg(test)]
    fn operation_id(&mut self, id: &OperationId) -> Result<(), SnapshotCodecError> {
        self.string(&id.process_id)?;
        self.string(id.task.node.as_str())?;
        self.u64(id.task.generation);
        self.u64(id.sequence);
        Ok(())
    }
    fn wire_task_id(&mut self, id: &TaskId, process_id: &str) -> Result<(), SnapshotCodecError> {
        self.string(&id.wire(process_id))?;
        self.string(id.node.as_str())?;
        self.u64(id.generation);
        Ok(())
    }
    fn wire_optional_task(
        &mut self,
        id: Option<&TaskId>,
        process_id: &str,
    ) -> Result<(), SnapshotCodecError> {
        match id {
            Some(id) => {
                self.u8(1);
                self.wire_task_id(id, process_id)?;
            }
            None => self.u8(0),
        }
        Ok(())
    }
    fn wire_operation_id(&mut self, id: &OperationId) -> Result<(), SnapshotCodecError> {
        self.string(&id.process_id)?;
        self.string(&id.task.wire(&id.process_id))?;
        self.string(id.task.node.as_str())?;
        self.u64(id.task.generation);
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
    #[cfg(test)]
    fn operation(&mut self, operation: &OperationRecord) -> Result<(), SnapshotCodecError> {
        self.operation_id(&operation.id)?;
        self.u8(operation_kind_tag(operation.kind));
        self.u64(operation.revision);
        self.operation_state(&operation.state)
    }

    fn wire_operation(&mut self, operation: &OperationRecord) -> Result<(), SnapshotCodecError> {
        self.wire_operation_id(&operation.id)?;
        self.u8(operation_kind_tag(operation.kind));
        self.u64(operation.revision);
        self.operation_state(&operation.state)
    }

    /// One task record, in the V3 shape: the base snapshot and every log
    /// entry use this exact layout, so applying a log record cannot drift from
    /// decoding a whole snapshot.
    fn task_record(
        &mut self,
        id: &TaskId,
        value: &TaskSnapshot,
        process_id: &str,
    ) -> Result<(), SnapshotCodecError> {
        self.wire_task_id(id, process_id)?;
        self.string(value.model_node.as_str())?;
        self.wire_optional_task(value.parent.as_ref(), process_id)?;
        let mut predecessors = value.predecessors.clone();
        predecessors.sort();
        self.count(predecessors.len())?;
        for predecessor in &predecessors {
            self.wire_task_id(predecessor, process_id)?;
        }
        self.u8(task_state_tag(value.state));
        self.u64(value.revision);
        Ok(())
    }

    fn variable_record(
        &mut self,
        scope: &TaskId,
        name: &str,
        value: &str,
        process_id: &str,
    ) -> Result<(), SnapshotCodecError> {
        self.wire_task_id(scope, process_id)?;
        self.string(name)?;
        self.string(value)?;
        Ok(())
    }

    fn outbox_record(
        &mut self,
        record: &crate::scheduler::core::outbox::records::OutboxRecord,
    ) -> Result<(), SnapshotCodecError> {
        let bytes = crate::scheduler::core::outbox::outbox_codec::OutboxCodec::encode(record)
            .map_err(|error| SnapshotCodecError::InvalidOutbox(error.to_string()))?;
        self.bytes(&bytes)
    }

    /// The lifecycle state both document versions encode identically.
    fn operation_state(&mut self, state: &OperationState) -> Result<(), SnapshotCodecError> {
        match state {
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
            task: TaskId::new(self.string()?, self.u64()?)
                .map_err(|_| SnapshotCodecError::InvalidTaskId)?,
            sequence: self.u64()?,
        })
    }
    /// The V3 identity: the wire string rides along (the instance id as
    /// rendered at write time) but the `TaskId` rebuilds from the node string
    /// and the generation — the derivation is pure, so the two agree by
    /// construction, and the read path never recomputes the digest.
    fn wire_task_id(&mut self) -> Result<TaskId, SnapshotCodecError> {
        let _wire = self.string()?;
        TaskId::new(self.string()?, self.u64()?).map_err(|_| SnapshotCodecError::InvalidTaskId)
    }
    fn wire_optional_task(&mut self) -> Result<Option<TaskId>, SnapshotCodecError> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.wire_task_id()?)),
            value => Err(SnapshotCodecError::InvalidTag(value)),
        }
    }
    fn wire_operation_id(&mut self) -> Result<OperationId, SnapshotCodecError> {
        let process_id = self.string()?;
        let _wire = self.string()?;
        Ok(OperationId {
            process_id,
            task: TaskId::new(self.string()?, self.u64()?)
                .map_err(|_| SnapshotCodecError::InvalidTaskId)?,
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
        Ok(OperationRecord {
            id,
            kind,
            state: self.operation_state()?,
            revision,
        })
    }

    fn wire_operation(&mut self) -> Result<OperationRecord, SnapshotCodecError> {
        let id = self.wire_operation_id()?;
        let kind = decode_operation_kind(self.u8()?)?;
        let revision = self.u64()?;
        Ok(OperationRecord {
            id,
            kind,
            state: self.operation_state()?,
            revision,
        })
    }

    fn task_record(&mut self) -> Result<(TaskId, TaskSnapshot), SnapshotCodecError> {
        let id = self.wire_task_id()?;
        let model_node =
            NodeId::parse(self.string()?).map_err(|_| SnapshotCodecError::InvalidNodeId)?;
        let parent = self.wire_optional_task()?;
        let predecessor_count = self.count_bounded(MIN_EXECUTION_ID_BYTES_V3)?;
        let mut predecessors = Vec::with_capacity(predecessor_count);
        for _ in 0..predecessor_count {
            predecessors.push(self.wire_task_id()?);
        }
        let state = decode_task_state(self.u8()?)?;
        let revision = self.u64()?;
        Ok((
            id,
            TaskSnapshot {
                model_node,
                parent,
                predecessors,
                state,
                revision,
            },
        ))
    }

    fn variable_record(&mut self) -> Result<((TaskId, String), String), SnapshotCodecError> {
        let scope = self.wire_task_id()?;
        let name = self.string()?;
        let value = self.string()?;
        Ok(((scope, name), value))
    }

    fn outbox_record(
        &mut self,
    ) -> Result<crate::scheduler::core::outbox::records::OutboxRecord, SnapshotCodecError> {
        crate::scheduler::core::outbox::outbox_codec::OutboxCodec::decode(self.bytes()?)
            .map_err(|error| SnapshotCodecError::InvalidOutbox(error.to_string()))
    }

    /// The lifecycle state both document versions encode identically.
    fn operation_state(&mut self) -> Result<OperationState, SnapshotCodecError> {
        Ok(match self.u8()? {
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
        ReturnToParent => 10,
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
        10 => Ok(ReturnToParent),
        value => Err(SnapshotCodecError::InvalidTag(value)),
    }
}
fn operation_id_order(left: &OperationId, right: &OperationId) -> std::cmp::Ordering {
    left.process_id
        .cmp(&right.process_id)
        .then_with(|| left.task.node.cmp(&right.task.node))
        .then_with(|| left.task.generation.cmp(&right.task.generation))
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
    ChangedRecordMissing(String),
    InvalidLog(String),
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
    use crate::scheduler::core::outbox::records::{OutboxId, OutboxRecord};

    fn encoded_u32(bytes: &[u8], offset: usize) -> u32 {
        let field = bytes
            .get(offset..offset + 4)
            .expect("test fixture contains the requested u32 field");
        u32::from_be_bytes(field.try_into().expect("test fixture u32 has four bytes"))
    }
    use crate::scheduler::core::commit::aggregate::MemoryProcessRepository;
    use crate::scheduler::core::commit::aggregate::{
        MemoryProcessRepositoryAccess, ProcessRepository,
    };

    fn snapshot() -> ProcessSnapshot {
        let root = TaskId::new("root", 0).unwrap();
        let child = TaskId::new("child", 1).unwrap();
        let operation_id = OperationId {
            process_id: "p".into(),
            task: child.clone(),
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
        let encoded = SnapshotCodecV3::encode(&source).unwrap();
        assert_eq!(SnapshotCodecV3::decode(&encoded).unwrap(), source);
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
            SnapshotCodecV3::encode(&source).unwrap(),
            SnapshotCodecV3::encode(&reordered).unwrap()
        );
    }

    #[test]
    fn truncated_and_trailing_payloads_are_rejected() {
        let encoded = SnapshotCodecV3::encode(&snapshot()).unwrap();
        for length in 0..encoded.len() {
            assert!(SnapshotCodecV3::decode(&encoded[..length]).is_err());
        }
        let mut trailing = encoded;
        trailing.push(0);
        assert_eq!(
            SnapshotCodecV3::decode(&trailing),
            Err(SnapshotCodecError::TrailingBytes)
        );
    }

    #[test]
    fn decoded_snapshot_passes_repository_validation_boundary() {
        let decoded =
            SnapshotCodecV3::decode(&SnapshotCodecV3::encode(&snapshot()).unwrap()).unwrap();
        let mut repository = MemoryProcessRepository::default();
        repository.insert_for_test(decoded).unwrap();
        assert!(repository.load("p").unwrap().is_some());
    }
    #[test]
    fn impossible_task_count_is_rejected_before_allocation() {
        let mut encoded = SnapshotCodecV3::encode(&snapshot()).unwrap();
        let process_length = usize::try_from(encoded_u32(&encoded, 8)).unwrap();
        let task_count_offset = 12 + process_length + 24;
        encoded[task_count_offset..task_count_offset + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(
            SnapshotCodecV3::decode(&encoded),
            Err(SnapshotCodecError::Truncated)
        );
    }
    /// The V3 golden (C5): the frozen wire format once the identity carries
    /// its wire. The V2 golden this test replaced (286 bytes,
    /// `26dc1279e7f4eddd825ee0c655877f26236ad360ba3ae62adcfac54168b7e693`)
    /// is preserved one test down as the upcast anchor.
    #[test]
    fn snapshot_wire_format_matches_golden_fingerprint() {
        let encoded = SnapshotCodecV3::encode(&snapshot()).unwrap();
        assert_eq!(encoded.len(), 502);
        assert_eq!(
            crate::scheduler::core::commit::aggregate::sha256_hex(&encoded),
            "4887924dd93a41c07aa4518a2da3a19f85e48073ec243d299cae3d66219ff83a"
        );
    }
    fn task_snapshot(model: &str) -> TaskSnapshot {
        TaskSnapshot {
            model_node: NodeId::parse(model).unwrap(),
            parent: None,
            predecessors: vec![],
            state: TaskState::Ready,
            revision: 0,
        }
    }

    fn outbox_record(process_id: &str, key: &str) -> OutboxRecord {
        OutboxRecord::pending(
            OutboxId {
                process_id: process_id.into(),
                key: key.into(),
            },
            "topic".into(),
            "{}".into(),
            0,
        )
        .unwrap()
    }

    fn v2_header(process_id: &str) -> Writer {
        let mut writer = Writer::default();
        writer.raw(PAYLOAD_MAGIC);
        writer.string(process_id).unwrap();
        writer.u64(0);
        writer.u64(0);
        writer.u64(0);
        writer
    }

    fn v3_header(process_id: &str) -> Writer {
        let mut writer = Writer::default();
        writer.raw(PAYLOAD_MAGIC_V3);
        writer.string(process_id).unwrap();
        writer.u64(0);
        writer.u64(0);
        writer.u64(0);
        writer
    }

    fn log_header(process_id: &str, sequence: u64) -> Writer {
        let mut writer = Writer::default();
        writer.raw(PAYLOAD_MAGIC_V4_LOG);
        writer.string(process_id).unwrap();
        writer.u64(sequence);
        writer
    }

    /// The V2 writer/reader pair still carries outbox records (the fixture
    /// format predates the L8 log), and every duplicate a hostile payload can
    /// express is rejected rather than silently overwriting a record.
    #[test]
    fn v2_outbox_round_trips_and_duplicates_are_rejected() {
        let mut source = snapshot();
        let record = outbox_record("p", "k");
        source.outbox.insert(record.id.clone(), record);
        let encoded = SnapshotCodecV2::encode(&source).unwrap();
        assert_eq!(SnapshotCodecV2::decode(&encoded).unwrap(), source);

        let id = TaskId::new("dup", 0).unwrap();
        let task = task_snapshot("model-dup");
        let mut duplicate_task = v2_header("p");
        duplicate_task.count(2).unwrap();
        for _ in 0..2 {
            duplicate_task.task_id(&id).unwrap();
            duplicate_task.string(task.model_node.as_str()).unwrap();
            duplicate_task.optional_task(None).unwrap();
            duplicate_task.count(0).unwrap();
            duplicate_task.u8(task_state_tag(TaskState::Ready));
            duplicate_task.u64(0);
        }
        assert_eq!(
            SnapshotCodecV2::decode(&duplicate_task.bytes),
            Err(SnapshotCodecError::DuplicateTask(id.clone()))
        );

        let mut duplicate_variable = v2_header("p");
        duplicate_variable.count(0).unwrap();
        duplicate_variable.count(2).unwrap();
        for _ in 0..2 {
            duplicate_variable.task_id(&id).unwrap();
            duplicate_variable.string("name").unwrap();
            duplicate_variable.string("value").unwrap();
        }
        assert_eq!(
            SnapshotCodecV2::decode(&duplicate_variable.bytes),
            Err(SnapshotCodecError::DuplicateVariable(
                id.clone(),
                "name".into()
            ))
        );

        let operation_id = OperationId {
            process_id: "p".into(),
            task: id.clone(),
            sequence: 3,
        };
        let operation = OperationRecord {
            id: operation_id.clone(),
            kind: OperationKind::ExecuteTask,
            state: OperationState::Pending,
            revision: 1,
        };
        let mut duplicate_operation = v2_header("p");
        duplicate_operation.count(0).unwrap();
        duplicate_operation.count(0).unwrap();
        duplicate_operation.count(2).unwrap();
        for _ in 0..2 {
            duplicate_operation.operation(&operation).unwrap();
        }
        assert_eq!(
            SnapshotCodecV2::decode(&duplicate_operation.bytes),
            Err(SnapshotCodecError::DuplicateOperation(operation_id))
        );

        let duplicate = outbox_record("p", "same");
        let bytes =
            crate::scheduler::core::outbox::outbox_codec::OutboxCodec::encode(&duplicate).unwrap();
        let mut duplicate_outbox = v2_header("p");
        duplicate_outbox.count(0).unwrap();
        duplicate_outbox.count(0).unwrap();
        duplicate_outbox.count(0).unwrap();
        duplicate_outbox.count(2).unwrap();
        for _ in 0..2 {
            duplicate_outbox.bytes(&bytes).unwrap();
        }
        assert_eq!(
            SnapshotCodecV2::decode(&duplicate_outbox.bytes),
            Err(SnapshotCodecError::DuplicateOutboxKey("same".into()))
        );

        let mut trailing = SnapshotCodecV2::encode(&snapshot()).unwrap();
        trailing.push(0);
        assert_eq!(
            SnapshotCodecV2::decode(&trailing),
            Err(SnapshotCodecError::TrailingBytes)
        );
    }

    /// The V3 reader applies the same duplicate checks to the wire-shaped
    /// records a committed snapshot can contain.
    #[test]
    fn v3_duplicate_records_are_rejected() {
        let id = TaskId::new("dup", 0).unwrap();
        let task = task_snapshot("model-dup");

        let mut duplicate_task = v3_header("p");
        duplicate_task.count(2).unwrap();
        for _ in 0..2 {
            duplicate_task.task_record(&id, &task, "p").unwrap();
        }
        assert_eq!(
            SnapshotCodecV3::decode(&duplicate_task.bytes),
            Err(SnapshotCodecError::DuplicateTask(id.clone()))
        );

        let mut duplicate_variable = v3_header("p");
        duplicate_variable.count(0).unwrap();
        duplicate_variable.count(2).unwrap();
        for _ in 0..2 {
            duplicate_variable
                .variable_record(&id, "name", "value", "p")
                .unwrap();
        }
        assert_eq!(
            SnapshotCodecV3::decode(&duplicate_variable.bytes),
            Err(SnapshotCodecError::DuplicateVariable(
                id.clone(),
                "name".into()
            ))
        );

        let operation_id = OperationId {
            process_id: "p".into(),
            task: id.clone(),
            sequence: 3,
        };
        let operation = OperationRecord {
            id: operation_id.clone(),
            kind: OperationKind::ExecuteTask,
            state: OperationState::Pending,
            revision: 1,
        };
        let mut duplicate_operation = v3_header("p");
        duplicate_operation.count(0).unwrap();
        duplicate_operation.count(0).unwrap();
        duplicate_operation.count(2).unwrap();
        for _ in 0..2 {
            duplicate_operation.wire_operation(&operation).unwrap();
        }
        assert_eq!(
            SnapshotCodecV3::decode(&duplicate_operation.bytes),
            Err(SnapshotCodecError::DuplicateOperation(operation_id))
        );

        let duplicate = outbox_record("p", "same");
        let mut duplicate_outbox = v3_header("p");
        duplicate_outbox.count(0).unwrap();
        duplicate_outbox.count(0).unwrap();
        duplicate_outbox.count(0).unwrap();
        duplicate_outbox.count(2).unwrap();
        for _ in 0..2 {
            duplicate_outbox.outbox_record(&duplicate).unwrap();
        }
        assert_eq!(
            SnapshotCodecV3::decode(&duplicate_outbox.bytes),
            Err(SnapshotCodecError::DuplicateOutboxKey("same".into()))
        );
    }

    /// Collecting a log entry from changes that name records the snapshot does
    /// not have is a caller bug, not an empty entry.
    #[test]
    fn log_collection_reports_every_missing_record_kind() {
        let source = snapshot();
        let absent_task = TaskId::new("absent", 0).unwrap();

        let mut changes = LogChanges::default();
        changes.tasks.insert(absent_task.clone());
        let missing = LogPayload::collect(&source, 0, &changes)
            .unwrap_err()
            .to_string();
        assert!(missing.contains("task"));

        let mut changes = LogChanges::default();
        changes
            .variables
            .insert((absent_task.clone(), "missing".into()));
        let missing = LogPayload::collect(&source, 0, &changes)
            .unwrap_err()
            .to_string();
        assert!(missing.contains("variable"));

        let mut changes = LogChanges::default();
        changes.operations.insert(OperationId {
            process_id: "p".into(),
            task: absent_task.clone(),
            sequence: 1,
        });
        let missing = LogPayload::collect(&source, 0, &changes)
            .unwrap_err()
            .to_string();
        assert!(missing.contains("operation"));

        let mut changes = LogChanges::default();
        changes.outbox.insert(OutboxId {
            process_id: "p".into(),
            key: "missing".into(),
        });
        let missing = LogPayload::collect(&source, 0, &changes)
            .unwrap_err()
            .to_string();
        assert!(missing.contains("outbox"));
    }

    fn head(base_seq: u64, next_seq: u64) -> HeadPayload {
        HeadPayload {
            process_id: "p".into(),
            version: ProcessVersion {
                graph: 1,
                process: 2,
                operations: 3,
            },
            epoch: 4,
            base_seq,
            next_seq,
        }
    }

    /// The head writer and reader both validate the log window, so an inverted
    /// window cannot be persisted or loaded.
    #[test]
    fn head_codec_rejects_invalid_windows_and_documents() {
        let empty_process = SnapshotCodecV4::encode_head(&HeadPayload {
            process_id: "   ".into(),
            ..head(0, 0)
        })
        .unwrap_err()
        .to_string();
        assert!(empty_process.contains("empty process id"));
        let inverted_window = SnapshotCodecV4::encode_head(&head(2, 1))
            .unwrap_err()
            .to_string();
        assert!(inverted_window.contains("inverted"));

        let mut invalid_magic = SnapshotCodecV4::encode_head(&head(0, 2)).unwrap();
        invalid_magic[0] ^= 0xff;
        assert_eq!(
            SnapshotCodecV4::decode_head(&invalid_magic),
            Err(SnapshotCodecError::InvalidMagic)
        );

        let mut trailing = SnapshotCodecV4::encode_head(&head(0, 2)).unwrap();
        trailing.push(0);
        assert_eq!(
            SnapshotCodecV4::decode_head(&trailing),
            Err(SnapshotCodecError::TrailingBytes)
        );

        let mut empty = Writer::default();
        empty.raw(PAYLOAD_MAGIC_V4_HEAD);
        empty.string("").unwrap();
        empty.u64(0);
        empty.u64(0);
        empty.u64(0);
        empty.u64(0);
        empty.u64(0);
        empty.u64(0);
        let empty_process = SnapshotCodecV4::decode_head(&empty.bytes)
            .unwrap_err()
            .to_string();
        assert!(empty_process.contains("empty process id"));

        let mut inverted = Writer::default();
        inverted.raw(PAYLOAD_MAGIC_V4_HEAD);
        inverted.string("p").unwrap();
        inverted.u64(0);
        inverted.u64(0);
        inverted.u64(0);
        inverted.u64(0);
        inverted.u64(2);
        inverted.u64(1);
        let inverted_window = SnapshotCodecV4::decode_head(&inverted.bytes)
            .unwrap_err()
            .to_string();
        assert!(inverted_window.contains("inverted"));
    }

    /// A bounded collection count must both fit the remaining bytes and carry
    /// a positive per-item minimum; a zero minimum would divide by zero.
    #[test]
    fn reader_rejects_zero_minimum_item_size_and_overflowing_counts() {
        let zero = 0u32.to_be_bytes();
        let mut zero_minimum = Reader::new(&zero);
        assert_eq!(
            zero_minimum.count_bounded(0),
            Err(SnapshotCodecError::Truncated)
        );

        let two = 2u32.to_be_bytes();
        let mut too_many = Reader::new(&two);
        assert_eq!(
            too_many.count_bounded(4),
            Err(SnapshotCodecError::Truncated)
        );

        let empty = 0u32.to_be_bytes();
        let mut valid = Reader::new(&empty);
        assert_eq!(valid.count_bounded(4), Ok(0));
    }

    #[test]
    fn base_codec_rejects_foreign_magic_and_trailing_bytes() {
        let mut encoded = SnapshotCodecV4::encode_base(&snapshot(), 3).unwrap();
        encoded[0] ^= 0xff;
        assert_eq!(
            SnapshotCodecV4::decode_base(&encoded),
            Err(SnapshotCodecError::InvalidMagic)
        );

        let mut trailing = SnapshotCodecV4::encode_base(&snapshot(), 3).unwrap();
        trailing.push(0);
        assert_eq!(
            SnapshotCodecV4::decode_base(&trailing),
            Err(SnapshotCodecError::TrailingBytes)
        );
    }

    /// The log reader rejects a foreign magic, duplicate records and trailing
    /// bytes; each entry is self-contained and independently validated.
    #[test]
    fn log_codec_rejects_duplicates_and_trailing_bytes() {
        let source = snapshot();
        let changes = LogChanges {
            tasks: source.tasks.keys().cloned().collect(),
            variables: source.variables.keys().cloned().collect(),
            operations: source.operations.keys().cloned().collect(),
            outbox: source.outbox.keys().cloned().collect(),
        };
        let mut invalid_magic =
            SnapshotCodecV4::encode_log(&LogPayload::collect(&source, 0, &changes).unwrap())
                .unwrap();
        invalid_magic[0] ^= 0xff;
        assert_eq!(
            SnapshotCodecV4::decode_log(&invalid_magic),
            Err(SnapshotCodecError::InvalidMagic)
        );

        let id = TaskId::new("dup", 0).unwrap();
        let task = task_snapshot("model-dup");
        let mut duplicate_task = log_header("p", 0);
        duplicate_task.count(2).unwrap();
        for _ in 0..2 {
            duplicate_task.task_record(&id, &task, "p").unwrap();
        }
        assert_eq!(
            SnapshotCodecV4::decode_log(&duplicate_task.bytes),
            Err(SnapshotCodecError::DuplicateTask(id.clone()))
        );

        let mut duplicate_variable = log_header("p", 0);
        duplicate_variable.count(0).unwrap();
        duplicate_variable.count(2).unwrap();
        for _ in 0..2 {
            duplicate_variable
                .variable_record(&id, "name", "value", "p")
                .unwrap();
        }
        assert_eq!(
            SnapshotCodecV4::decode_log(&duplicate_variable.bytes),
            Err(SnapshotCodecError::DuplicateVariable(
                id.clone(),
                "name".into()
            ))
        );

        let operation_id = OperationId {
            process_id: "p".into(),
            task: id.clone(),
            sequence: 3,
        };
        let operation = OperationRecord {
            id: operation_id.clone(),
            kind: OperationKind::ExecuteTask,
            state: OperationState::Pending,
            revision: 1,
        };
        let mut duplicate_operation = log_header("p", 0);
        duplicate_operation.count(0).unwrap();
        duplicate_operation.count(0).unwrap();
        duplicate_operation.count(2).unwrap();
        for _ in 0..2 {
            duplicate_operation.wire_operation(&operation).unwrap();
        }
        assert_eq!(
            SnapshotCodecV4::decode_log(&duplicate_operation.bytes),
            Err(SnapshotCodecError::DuplicateOperation(operation_id))
        );

        let duplicate = outbox_record("p", "same");
        let mut duplicate_outbox = log_header("p", 0);
        duplicate_outbox.count(0).unwrap();
        duplicate_outbox.count(0).unwrap();
        duplicate_outbox.count(0).unwrap();
        duplicate_outbox.count(2).unwrap();
        for _ in 0..2 {
            duplicate_outbox.outbox_record(&duplicate).unwrap();
        }
        assert_eq!(
            SnapshotCodecV4::decode_log(&duplicate_outbox.bytes),
            Err(SnapshotCodecError::DuplicateOutboxKey("same".into()))
        );

        let mut trailing =
            SnapshotCodecV4::encode_log(&LogPayload::collect(&source, 0, &changes).unwrap())
                .unwrap();
        trailing.push(0);
        assert_eq!(
            SnapshotCodecV4::decode_log(&trailing),
            Err(SnapshotCodecError::TrailingBytes)
        );
    }

    /// Every lifecycle state, operation kind and task state has a distinct
    /// stored tag and survives a round trip.
    #[test]
    fn every_state_kind_and_tag_round_trips() {
        let mut source = snapshot();
        let root = TaskId::new("root", 0).unwrap();
        source.tasks.get_mut(&root).unwrap().state = TaskState::Cancelled;
        let removed = TaskId::new("removed", 2).unwrap();
        source.tasks.insert(
            removed.clone(),
            TaskSnapshot {
                state: TaskState::Removed,
                ..task_snapshot("model-removed")
            },
        );

        let claim = ClaimToken {
            owner: "runtime".into(),
            fence: 1,
            lease_until_ms: 10,
            heartbeat_revision: 0,
        };
        let commit = CommitToken {
            process_version: 5,
            digest: "digest".into(),
        };
        let states = [
            (OperationKind::ExecuteTask, OperationState::Pending),
            (
                OperationKind::DeliverMessage,
                OperationState::Claimed(claim.clone()),
            ),
            (
                OperationKind::FireTrigger,
                OperationState::CommitPrepared(claim.clone()),
            ),
            (
                OperationKind::RunPostCommit,
                OperationState::Committed(commit.clone()),
            ),
            (
                OperationKind::AbortTask,
                OperationState::RetryScheduled { attempt: 2 },
            ),
            (
                OperationKind::SkipTask,
                OperationState::EffectUnknown { attempt: 3 },
            ),
            (
                OperationKind::ApplyAction,
                OperationState::Quarantined {
                    reason: "review".into(),
                },
            ),
            (
                OperationKind::PropagateTask,
                OperationState::Superseded { by_generation: 7 },
            ),
            (
                OperationKind::DispatchTimeout,
                OperationState::Completed(commit.clone()),
            ),
            (OperationKind::ReenterTask, OperationState::Claimed(claim)),
            (
                OperationKind::ReturnToParent,
                OperationState::Completed(commit),
            ),
        ];
        for (sequence, (kind, state)) in states.into_iter().enumerate() {
            let id = OperationId {
                process_id: "p".into(),
                task: root.clone(),
                sequence: sequence as u64,
            };
            source.operations.insert(
                id.clone(),
                OperationRecord {
                    id,
                    kind,
                    state,
                    revision: sequence as u64,
                },
            );
        }
        let encoded = SnapshotCodecV3::encode(&source).unwrap();
        assert_eq!(SnapshotCodecV3::decode(&encoded).unwrap(), source);
    }

    /// Unknown tags are refused by every tagged reader instead of being mapped
    /// onto a neighbouring variant.
    #[test]
    fn unknown_tags_are_rejected() {
        assert_eq!(decode_task_state(7).unwrap(), TaskState::Cancelled);
        assert_eq!(decode_task_state(11).unwrap(), TaskState::Removed);
        assert_eq!(
            decode_task_state(12),
            Err(SnapshotCodecError::InvalidTag(12))
        );
        for (tag, expected) in [
            (3, OperationKind::DeliverMessage),
            (4, OperationKind::FireTrigger),
            (5, OperationKind::RunPostCommit),
            (7, OperationKind::AbortTask),
        ] {
            assert_eq!(decode_operation_kind(tag).unwrap(), expected);
            assert_eq!(operation_kind_tag(expected), tag);
        }
        assert_eq!(
            decode_operation_kind(10).unwrap(),
            OperationKind::ReturnToParent
        );
        assert_eq!(
            decode_operation_kind(11),
            Err(SnapshotCodecError::InvalidTag(11))
        );

        assert_eq!(
            Reader::new(&[2]).optional_task(),
            Err(SnapshotCodecError::InvalidTag(2))
        );
        assert_eq!(
            Reader::new(&[2]).wire_optional_task(),
            Err(SnapshotCodecError::InvalidTag(2))
        );
        assert_eq!(
            Reader::new(&[9]).operation_state(),
            Err(SnapshotCodecError::InvalidTag(9))
        );
        let mut superseded = [7u8; 9];
        superseded[1..].copy_from_slice(&7u64.to_be_bytes());
        assert_eq!(
            Reader::new(&superseded).operation_state().unwrap(),
            OperationState::Superseded { by_generation: 7 }
        );
    }

    #[test]
    fn codec_error_display_names_the_codec() {
        assert!(
            SnapshotCodecError::InvalidMagic
                .to_string()
                .contains("snapshot codec error")
        );
    }

    /// The upcast anchor: the V2 bytes are exactly what the pre-C5 kernel
    /// wrote (the old golden, unchanged), the V2 reader still loads them, and
    /// the V3 reader refuses them only by magic — which is what
    /// `ProcessDocumentCodec::decode` falls back on (document.rs).
    #[test]
    fn v2_documents_still_decode() {
        let source = snapshot();
        let encoded = SnapshotCodecV2::encode(&source).unwrap();
        assert_eq!(encoded.len(), 286);
        assert_eq!(
            crate::scheduler::core::commit::aggregate::sha256_hex(&encoded),
            "26dc1279e7f4eddd825ee0c655877f26236ad360ba3ae62adcfac54168b7e693"
        );
        assert_eq!(SnapshotCodecV2::decode(&encoded).unwrap(), source);
        assert_eq!(
            SnapshotCodecV3::decode(&encoded),
            Err(SnapshotCodecError::InvalidMagic)
        );
    }
}
