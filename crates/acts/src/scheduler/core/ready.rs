//! The durable ready index: the operations normal execution may drive.
//!
//! The index is written in the same atomic batch as the aggregate commit that
//! creates (or requeues) an operation, so a lost wake can never lose the work:
//! `QueueWake` in the shell is only an accelerator, and a crash either leaves
//! both the aggregate and its ready record or neither. Recovery
//! does not wake ordinary `Pending`/`RetryScheduled` operations any more — it
//! repairs this index and handles the exceptional states.
//!
//! The document is per process and carries one record per schedulable
//! operation. `ready_at_ms` is the moment the operation becomes due; execution
//! reads only due records. Timers and parked runs are indexed too, but with a
//! future `ready_at_ms`, so they never enter the hot set: a timer waits for its
//! next round and a parked run for the next admission check.

use crate::scheduler::core::{
    commit::aggregate::ProcessSnapshot,
    model::operation::{OperationId, OperationKind, OperationRecord, OperationState},
    recovery::recovery_index::operation_order,
};

use std::collections::HashMap;
use std::fmt;

const MAGIC: &[u8; 8] = b"ACTRDY\0\0";
const SCHEMA_V1: u16 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReadyEntry {
    pub(crate) operation_id: OperationId,
    pub(crate) ready_at_ms: i64,
    /// The aggregate operation revision this record was projected from. It is
    /// evidence for diagnostics and repair; execution trusts the aggregate's
    /// own operation record at claim time.
    pub(crate) source_aggregate_revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReadyIndex {
    pub(crate) process_id: String,
    pub(crate) entries: Vec<ReadyEntry>,
}

impl ReadyIndex {
    /// Project a fresh index from an aggregate — the create path and the boot
    /// repair path. Existing ready times are not known, so every operation gets
    /// its default: due now, or the next poll for a timer/parked root.
    pub(crate) fn project(snapshot: &ProcessSnapshot, now_ms: i64, poll_ms: i64) -> Option<Self> {
        Self::reconcile(None, snapshot, now_ms, poll_ms, &HashMap::new())
    }

    /// Reconcile an existing index with a committed snapshot.
    ///
    /// Existing entries keep their `ready_at_ms` (a timer's deferred round, a
    /// continuously pending operation) and their source revision; operations
    /// that are no longer schedulable disappear; newly schedulable operations
    /// take `ready_at_ms` from the plan when it supplied one, otherwise their
    /// default. The operation set is still read from the snapshot, so an old
    /// database with no ready document is repaired by the first write.
    pub(crate) fn reconcile(
        previous: Option<&ReadyIndex>,
        snapshot: &ProcessSnapshot,
        now_ms: i64,
        poll_ms: i64,
        ready_at: &HashMap<OperationId, i64>,
    ) -> Option<Self> {
        let previous = previous
            .map(|index| {
                index
                    .entries
                    .iter()
                    .map(|entry| (&entry.operation_id, entry))
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();
        let mut entries = Vec::new();
        for operation in snapshot.operations.values() {
            if !schedulable(operation) {
                continue;
            }
            let prior = previous.get(&operation.id);
            entries.push(ReadyEntry {
                operation_id: operation.id.clone(),
                ready_at_ms: ready_at
                    .get(&operation.id)
                    .copied()
                    .or_else(|| prior.map(|entry| entry.ready_at_ms))
                    .unwrap_or_else(|| default_ready_at(operation, now_ms, poll_ms)),
                source_aggregate_revision: prior
                    .map(|entry| entry.source_aggregate_revision)
                    .unwrap_or(snapshot.version.operations),
            });
        }
        if entries.is_empty() {
            return None;
        }
        entries.sort_by(|left, right| {
            left.ready_at_ms
                .cmp(&right.ready_at_ms)
                .then_with(|| operation_order(&left.operation_id, &right.operation_id))
        });
        Some(Self {
            process_id: snapshot.process_id.clone(),
            entries,
        })
    }

    /// The due records of one index, oldest first.
    #[cfg(test)]
    pub(crate) fn due(&self, now_ms: i64) -> impl Iterator<Item = &ReadyEntry> {
        self.entries
            .iter()
            .filter(move |entry| entry.ready_at_ms <= now_ms)
    }
}

/// Whether one operation belongs in the ready index at all.
///
/// The kind classification lives beside the enum
/// ([`OperationKind::is_schedulable`]); this only adds the state check.
pub(crate) fn schedulable(operation: &OperationRecord) -> bool {
    matches!(
        operation.state,
        OperationState::Pending | OperationState::RetryScheduled { .. }
    ) && operation.kind.is_schedulable()
}

/// The first moment a newly schedulable operation may run.
///
/// Ordinary work, including a parked run's root, is due immediately: the
/// slot decision itself decides whether it may run, and the driver defers a
/// parked run's record to the next poll when that answer is "wait". A
/// `DispatchTimeout` waits one poll interval: the legacy engine had no timer
/// round before the first process tick, and the timer evaluates its declared
/// windows then.
pub(crate) fn default_ready_at(operation: &OperationRecord, now_ms: i64, poll_ms: i64) -> i64 {
    if operation.kind == OperationKind::DispatchTimeout {
        now_ms.saturating_add(poll_ms.max(0))
    } else {
        now_ms
    }
}

pub(crate) struct ReadyIndexCodec;
impl ReadyIndexCodec {
    pub(crate) fn encode(index: &ReadyIndex) -> Result<Vec<u8>, ReadyIndexError> {
        validate(index)?;
        let mut output = Vec::new();
        output.extend_from_slice(MAGIC);
        output.extend_from_slice(&SCHEMA_V1.to_be_bytes());
        write_string(&mut output, &index.process_id)?;
        output.extend_from_slice(
            &u32::try_from(index.entries.len())
                .map_err(|_| ReadyIndexError::CollectionTooLarge)?
                .to_be_bytes(),
        );
        for entry in &index.entries {
            write_string(&mut output, &entry.operation_id.process_id)?;
            write_string(&mut output, entry.operation_id.task.node.as_str())?;
            output.extend_from_slice(&entry.operation_id.task.generation.to_be_bytes());
            output.extend_from_slice(&entry.operation_id.sequence.to_be_bytes());
            output.extend_from_slice(&entry.ready_at_ms.to_be_bytes());
            output.extend_from_slice(&entry.source_aggregate_revision.to_be_bytes());
        }
        Ok(output)
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<ReadyIndex, ReadyIndexError> {
        let mut reader = Reader { bytes, offset: 0 };
        if reader.take(MAGIC.len())? != MAGIC {
            return Err(ReadyIndexError::InvalidMagic);
        }
        let schema = reader.u16()?;
        if schema != SCHEMA_V1 {
            return Err(ReadyIndexError::UnsupportedSchema(schema));
        }
        let process_id = reader.string()?;
        let count =
            usize::try_from(reader.u32()?).map_err(|_| ReadyIndexError::CollectionTooLarge)?;
        // Two length prefixes and three u64 values per entry even with empty
        // strings; reject impossible counts before allocating.
        const MINIMUM_ENTRY_BYTES: usize = 4 + 4 + 8 + 8 + 8 + 8;
        if count > reader.remaining() / MINIMUM_ENTRY_BYTES {
            return Err(ReadyIndexError::Truncated);
        }
        let mut entries = Vec::with_capacity(count);
        let mut ids = std::collections::HashSet::with_capacity(count);
        for _ in 0..count {
            let operation_id = OperationId {
                process_id: reader.string()?,
                task: crate::scheduler::core::model::task_graph::TaskId::new(
                    reader.string()?,
                    reader.u64()?,
                )
                .map_err(|_| ReadyIndexError::InvalidTaskId)?,
                sequence: reader.u64()?,
            };
            let ready_at_ms = reader.i64()?;
            let source_aggregate_revision = reader.u64()?;
            if !ids.insert(operation_id.clone()) {
                return Err(ReadyIndexError::DuplicateOperation(operation_id));
            }
            entries.push(ReadyEntry {
                operation_id,
                ready_at_ms,
                source_aggregate_revision,
            });
        }
        if reader.offset != bytes.len() {
            return Err(ReadyIndexError::TrailingBytes);
        }
        let index = ReadyIndex {
            process_id,
            entries,
        };
        validate(&index)?;
        Ok(index)
    }
}

fn validate(index: &ReadyIndex) -> Result<(), ReadyIndexError> {
    if index.process_id.trim().is_empty() {
        return Err(ReadyIndexError::EmptyProcessId);
    }
    if index.entries.is_empty() {
        return Err(ReadyIndexError::EmptyIndex);
    }
    let mut previous: Option<&ReadyEntry> = None;
    for entry in &index.entries {
        if entry.operation_id.process_id != index.process_id {
            return Err(ReadyIndexError::ForeignOperation(
                entry.operation_id.clone(),
            ));
        }
        if let Some(previous) = previous {
            let order = previous
                .ready_at_ms
                .cmp(&entry.ready_at_ms)
                .then_with(|| operation_order(&previous.operation_id, &entry.operation_id));
            match order {
                std::cmp::Ordering::Less => {}
                std::cmp::Ordering::Equal => {
                    return Err(ReadyIndexError::DuplicateOperation(
                        entry.operation_id.clone(),
                    ));
                }
                std::cmp::Ordering::Greater => return Err(ReadyIndexError::NonCanonicalOrder),
            }
        }
        previous = Some(entry);
    }
    Ok(())
}

fn write_string(output: &mut Vec<u8>, value: &str) -> Result<(), ReadyIndexError> {
    output.extend_from_slice(
        &u32::try_from(value.len())
            .map_err(|_| ReadyIndexError::StringTooLarge)?
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
    fn take(&mut self, length: usize) -> Result<&'a [u8], ReadyIndexError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(ReadyIndexError::Truncated)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(ReadyIndexError::Truncated)?;
        self.offset = end;
        Ok(value)
    }
    fn u16(&mut self) -> Result<u16, ReadyIndexError> {
        let bytes = self.take(2)?;
        let mut array = [0_u8; 2];
        array.copy_from_slice(bytes);
        Ok(u16::from_be_bytes(array))
    }
    fn u32(&mut self) -> Result<u32, ReadyIndexError> {
        let bytes = self.take(4)?;
        let mut array = [0_u8; 4];
        array.copy_from_slice(bytes);
        Ok(u32::from_be_bytes(array))
    }
    fn u64(&mut self) -> Result<u64, ReadyIndexError> {
        let bytes = self.take(8)?;
        let mut array = [0_u8; 8];
        array.copy_from_slice(bytes);
        Ok(u64::from_be_bytes(array))
    }
    fn i64(&mut self) -> Result<i64, ReadyIndexError> {
        let bytes = self.take(8)?;
        let mut array = [0_u8; 8];
        array.copy_from_slice(bytes);
        Ok(i64::from_be_bytes(array))
    }
    fn remaining(&self) -> usize {
        self.bytes.len() - self.offset
    }
    fn string(&mut self) -> Result<String, ReadyIndexError> {
        let length =
            usize::try_from(self.u32()?).map_err(|_| ReadyIndexError::CollectionTooLarge)?;
        let bytes = self.take(length)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| ReadyIndexError::InvalidUtf8)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ReadyIndexError {
    InvalidMagic,
    UnsupportedSchema(u16),
    Truncated,
    TrailingBytes,
    InvalidUtf8,
    StringTooLarge,
    CollectionTooLarge,
    InvalidTaskId,
    EmptyProcessId,
    EmptyIndex,
    DuplicateOperation(OperationId),
    ForeignOperation(OperationId),
    NonCanonicalOrder,
}
impl fmt::Display for ReadyIndexError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ready index error: {self:?}")
    }
}
impl std::error::Error for ReadyIndexError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::core::{
        commit::aggregate::TaskSnapshot,
        model::{
            model_graph::NodeId, task_graph::TaskId, task_graph::TaskState,
            transition_plan::ProcessVersion,
        },
    };
    use std::collections::{BTreeMap, HashMap};

    fn snapshot() -> ProcessSnapshot {
        let task = TaskId::new("step", 0).unwrap();
        let operation = OperationId {
            process_id: "process".into(),
            task: task.clone(),
            sequence: 0,
        };
        ProcessSnapshot {
            process_id: "process".into(),
            version: ProcessVersion {
                graph: 1,
                process: 1,
                operations: 3,
            },
            tasks: HashMap::from([(
                task.clone(),
                TaskSnapshot {
                    model_node: NodeId::parse("model-step").unwrap(),
                    parent: None,
                    predecessors: vec![],
                    state: TaskState::Ready,
                    revision: 1,
                },
            )]),
            variables: BTreeMap::new(),
            operations: HashMap::from([(
                operation.clone(),
                OperationRecord::pending(operation, OperationKind::ExecuteTask),
            )]),
            outbox: HashMap::new(),
        }
    }

    #[test]
    fn project_indexes_a_runnable_operation() {
        let index = ReadyIndex::project(&snapshot(), 100, 10).unwrap();
        assert_eq!(index.entries.len(), 1);
        assert_eq!(index.entries[0].ready_at_ms, 100);
        assert_eq!(index.entries[0].source_aggregate_revision, 3);
        assert_eq!(index.due(100).count(), 1);
        assert_eq!(index.due(99).count(), 0);
    }

    #[test]
    fn timer_waits_for_the_next_poll_and_parked_root_is_due_now() {
        let mut parked = snapshot();
        parked.tasks.values_mut().next().unwrap().state = TaskState::Created;
        let index = ReadyIndex::project(&parked, 100, 10).unwrap();
        assert_eq!(index.entries[0].ready_at_ms, 100);
        assert_eq!(index.due(100).count(), 1);

        let mut timer = snapshot();
        timer.operations.values_mut().next().unwrap().kind = OperationKind::DispatchTimeout;
        assert_eq!(
            ReadyIndex::project(&timer, 100, 10).unwrap().entries[0].ready_at_ms,
            110
        );
    }

    #[test]
    fn reconcile_preserves_deferred_time_and_drops_terminal_operations() {
        let before = snapshot();
        let mut previous = ReadyIndex::project(&before, 100, 10).unwrap();
        previous.entries[0].ready_at_ms = 500;
        let mut after = snapshot();
        after.version.operations = 4;
        let reconciled =
            ReadyIndex::reconcile(Some(&previous), &after, 200, 10, &HashMap::new()).unwrap();
        assert_eq!(reconciled.entries[0].ready_at_ms, 500);
        assert_eq!(reconciled.entries[0].source_aggregate_revision, 3);

        let id = reconciled.entries[0].operation_id.clone();
        let claim = crate::scheduler::core::model::operation::ClaimToken {
            owner: "owner".into(),
            fence: 1,
            lease_until_ms: 1_000,
            heartbeat_revision: 0,
        };
        after.operations.insert(
            id.clone(),
            after.operations[&id]
                .clone()
                .evolve(
                    crate::scheduler::core::model::operation::OperationEvent::ClaimAndPrepare(
                        claim,
                    ),
                )
                .unwrap(),
        );
        assert!(
            ReadyIndex::reconcile(Some(&reconciled), &after, 200, 10, &HashMap::new()).is_none()
        );
    }

    #[test]
    fn codec_round_trips_and_rejects_duplicates() {
        let index = ReadyIndex::project(&snapshot(), 100, 10).unwrap();
        let bytes = ReadyIndexCodec::encode(&index).unwrap();
        assert_eq!(ReadyIndexCodec::decode(&bytes).unwrap(), index);

        let mut duplicate = index.clone();
        duplicate.entries.push(duplicate.entries[0].clone());
        assert!(matches!(
            ReadyIndexCodec::encode(&duplicate),
            Err(ReadyIndexError::DuplicateOperation(_))
        ));
    }
}
