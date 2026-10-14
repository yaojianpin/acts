//! Durable repository adapter for the production runtime.
//!
//! The deterministic in-memory repository remains synchronous for exhaustive
//! state-machine tests. Production storage is async and must never be hidden
//! behind `block_on`, `block_in_place`, or a dedicated thread masquerading as
//! a synchronous repository. This port is the migration boundary.

use crate::{
    ActError,
    scheduler::{
        core::{
            commit::aggregate::{
                CommitError, CommitReceipt, RecoveryCursor, RecoveryPage, advance_operation_event,
                advance_transition,
            },
            outbox::{
                outbox_index::{OutboxIndexCodec, OutboxIndexState},
                outbox_repository::{OutboxCursor, OutboxPage, OutboxRepositoryError},
                records::{OutboxEvent, OutboxId, OutboxRecord},
            },
            recovery::recovery_index::{RecoveryIndexCodec, operation_order},
        },
        durable::document_store::{
            DocumentGuard, DocumentMutation, DocumentRepositoryError, LoadedAggregate,
            OUTBOX_INDEX_KEY_PREFIX, RECOVERY_INDEX_KEY_PREFIX, aggregate_key,
            build_create_transaction, build_replace_transaction, decode_loaded_aggregate,
            loaded_after_write, outbox_index_key, recovery_entries, recovery_index_key,
        },
    },
    store::{KvStore, ScanOperation, ScanOptions, StoreBatchOp, StoreGuard},
};

use std::collections::HashMap;
use std::sync::Arc;

const MAX_RECOVERY_PAGE_SIZE: usize = 1_024;
/// How many times a load re-reads after finding an aggregate paired with its
/// derived indexes from another version (see [`AsyncAggregateRepository::load`]).
/// Bounded: a store that keeps serving an inconsistent triple is damage, and
/// the caller must see it.
const LOAD_ATTEMPTS: usize = 4;

#[async_trait::async_trait]
pub(crate) trait AsyncDocumentStore: Send + Sync {
    async fn read_many(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>, ActError>;

    /// Return a stable key-ordered snapshot under `prefix`.
    async fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, ActError>;

    /// Apply every mutation atomically if and only if all guards still match.
    /// `Ok(false)` means no mutation became visible.
    async fn atomic_batch(
        &self,
        guards: &[DocumentGuard],
        mutations: &[DocumentMutation],
    ) -> Result<bool, ActError>;
}

/// Zero-blocking adapter from the engine's async storage contract to the
/// kernel's async document port.
pub(crate) struct KvDocumentStore {
    inner: Arc<dyn KvStore>,
}

impl KvDocumentStore {
    pub(crate) fn new(inner: Arc<dyn KvStore>) -> Self {
        Self { inner }
    }
}

#[async_trait::async_trait]
impl AsyncDocumentStore for KvDocumentStore {
    async fn read_many(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>, ActError> {
        self.inner.many(keys).await
    }

    async fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, ActError> {
        let mut entries = self
            .inner
            .scan_prefix(
                prefix,
                ScanOptions::new(ScanOperation::Eq, prefix.to_owned(), false),
            )
            .await?;
        entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        Ok(entries)
    }

    async fn atomic_batch(
        &self,
        guards: &[DocumentGuard],
        mutations: &[DocumentMutation],
    ) -> Result<bool, ActError> {
        let guards = guards
            .iter()
            .map(|guard| StoreGuard {
                key: guard.key.clone(),
                expected: guard.expected.clone(),
            })
            .collect::<Vec<_>>();
        let mutations = mutations
            .iter()
            .map(|mutation| match mutation {
                DocumentMutation::Put { key, value } => StoreBatchOp::Put {
                    key: key.clone(),
                    value: value.clone(),
                },
                DocumentMutation::Delete { key } => StoreBatchOp::Delete { key: key.clone() },
            })
            .collect::<Vec<_>>();
        self.inner.batch(&mutations, &guards).await
    }
}

/// Async aggregate repository for production I/O. It batch-loads the aggregate
/// and both derived indexes in one round trip, then delegates all integrity
/// checks to the same decoder used by the deterministic synchronous repository.
pub(crate) struct AsyncAggregateRepository {
    store: Arc<dyn AsyncDocumentStore>,
}

impl AsyncAggregateRepository {
    pub(crate) fn new(store: Arc<dyn AsyncDocumentStore>) -> Self {
        Self { store }
    }

    pub(crate) async fn create(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::AggregateSnapshot,
    ) -> Result<LoadedAggregate, AsyncRepositoryError> {
        let (transaction, document, recovery_index, outbox_index) =
            build_create_transaction(snapshot)?;
        if !self
            .store
            .atomic_batch(transaction.guards(), transaction.mutations())
            .await?
        {
            return Err(AsyncRepositoryError::Conflict(snapshot.process_id.clone()));
        }
        Ok(loaded_after_write(
            snapshot,
            document,
            recovery_index,
            outbox_index,
        ))
    }

    /// Read a stable page from the compact recovery projection, then validate
    /// every selected row against its authoritative aggregate before exposing it.
    async fn load_recovery_page(
        &self,
        after: Option<&RecoveryCursor>,
        limit: usize,
    ) -> Result<RecoveryPage, AsyncRepositoryError> {
        if limit == 0 || limit > MAX_RECOVERY_PAGE_SIZE {
            return Err(AsyncRepositoryError::Domain(CommitError::InvalidPageLimit));
        }
        // Keep only the smallest `limit + 1` entries after the cursor. This
        // bounds memory independently of the number of recoverable operations
        // while preserving deterministic global OperationId ordering.
        let candidate_limit = limit + 1;
        let mut entries = Vec::with_capacity(candidate_limit);
        for (key, bytes) in self.store.scan_prefix(RECOVERY_INDEX_KEY_PREFIX).await? {
            let index = RecoveryIndexCodec::decode(&bytes).map_err(|source| {
                AsyncRepositoryError::Domain(CommitError::CorruptAggregate(format!(
                    "recovery index {key:?}: {source}",
                )))
            })?;
            for entry in index.entries {
                if after
                    .is_some_and(|cursor| !operation_order(&entry.operation_id, &cursor.0).is_gt())
                {
                    continue;
                }
                insert_recovery_candidate(&mut entries, entry, candidate_limit);
            }
        }
        let has_more = entries.len() > limit;
        entries.truncate(limit);

        let mut aggregates = HashMap::new();
        let mut operations = Vec::with_capacity(entries.len());
        for entry in entries {
            let process_id = entry.operation_id.process_id.clone();
            if !aggregates.contains_key(&process_id) {
                // The aggregates are loaded at a later instant than the index
                // documents the entries came from, so the two can disagree: a
                // run swept (or a commit landing) in between leaves the index
                // the older view. That is the scan being stale, not damage —
                // and the projection is written atomically with its aggregate,
                // so a disagreement can only be this race. The entry is
                // skipped; the next pass re-reads the index.
                match self.load(&process_id).await? {
                    Some(loaded) => {
                        aggregates.insert(process_id.clone(), loaded);
                    }
                    None => continue,
                }
            }
            let aggregate = aggregates.get(&process_id).ok_or_else(|| {
                AsyncRepositoryError::Domain(CommitError::CorruptAggregate(format!(
                    "aggregate cache lost process {process_id:?}",
                )))
            })?;
            let Some(operation) = aggregate.snapshot.operations.get(&entry.operation_id) else {
                // The operation left the aggregate (it settled, and a terminal
                // operation is dropped from the index in the same commit) after
                // the index doc was scanned: stale, not damage.
                continue;
            };
            if operation.revision != entry.revision || operation.is_terminal() {
                // Same: the operation moved on after the index was scanned.
                continue;
            }
            operations.push(operation.clone());
        }
        let next = if has_more {
            operations
                .last()
                .map(|operation| RecoveryCursor(operation.id.clone()))
        } else {
            None
        };
        Ok(RecoveryPage { operations, next })
    }

    /// Apply one operation lifecycle event and atomically persist the
    /// operation, aggregate operation version and all derived indexes.
    pub(crate) async fn commit_operation_event(
        &self,
        id: &crate::scheduler::core::model::operation::OperationId,
        expected_revision: u64,
        event: crate::scheduler::core::model::operation::OperationEvent,
    ) -> Result<OperationCommitOutcome, AsyncRepositoryError> {
        let mut loaded = self.load(&id.process_id).await?.ok_or_else(|| {
            AsyncRepositoryError::Domain(CommitError::AggregateNotFound(id.process_id.clone()))
        })?;
        // The advance runs on the loaded aggregate itself: this repository owns
        // it and drops it on failure (the caller reports and recovery
        // re-derives the hop), so the whole-aggregate clone is not needed.
        let entries = recovery_entries(&loaded.snapshot);
        let operation = advance_operation_event(&mut loaded.snapshot, id, expected_revision, event)
            .map_err(AsyncRepositoryError::Domain)?;
        let outcome = self.replace(&loaded, &loaded.snapshot, &entries).await?;
        match outcome {
            ReplaceOutcome::Committed => Ok(OperationCommitOutcome::Committed {
                operation,
                snapshot: loaded.snapshot,
            }),
            ReplaceOutcome::AlreadyCommitted(aggregate) => {
                let operation = authoritative_operation(&aggregate, id)?;
                Ok(OperationCommitOutcome::AlreadyCommitted {
                    operation,
                    snapshot: aggregate.snapshot.clone(),
                })
            }
            ReplaceOutcome::Conflict => Ok(OperationCommitOutcome::Conflict {}),
        }
    }

    /// Apply a pure domain transition and persist the aggregate plus both
    /// derived indexes through one guarded transaction.
    pub(crate) async fn commit_transition(
        &self,
        plan: &crate::scheduler::core::model::transition_plan::TransitionPlan,
    ) -> Result<TransitionCommitOutcome, AsyncRepositoryError> {
        let mut loaded = self.load(&plan.process_id).await?.ok_or_else(|| {
            AsyncRepositoryError::Domain(CommitError::AggregateNotFound(plan.process_id.clone()))
        })?;
        // In place, like the operation event above.
        let entries = recovery_entries(&loaded.snapshot);
        let receipt =
            advance_transition(&mut loaded.snapshot, plan).map_err(AsyncRepositoryError::Domain)?;
        let outcome = self.replace(&loaded, &loaded.snapshot, &entries).await?;
        match outcome {
            ReplaceOutcome::Committed => Ok(TransitionCommitOutcome::Committed {
                receipt,
                snapshot: loaded.snapshot,
            }),
            ReplaceOutcome::AlreadyCommitted(aggregate) => {
                Ok(TransitionCommitOutcome::AlreadyCommitted {
                    receipt,
                    snapshot: aggregate.snapshot.clone(),
                })
            }
            ReplaceOutcome::Conflict => Ok(TransitionCommitOutcome::Conflict {}),
        }
    }

    /// Mirror of the synchronous repository's outbox paging: scan the derived
    /// per-process index documents, keep due records in canonical
    /// [`OutboxId`] order, and verify every entry against its authoritative
    /// aggregate before handing it out.
    pub(crate) async fn dispatchable_outbox_page(
        &self,
        now_ms: i64,
        after: Option<&OutboxCursor>,
        limit: usize,
    ) -> Result<OutboxPage, OutboxRepositoryError> {
        if limit == 0 {
            return Err(OutboxRepositoryError::InvalidPageLimit);
        }
        let documents = self
            .store
            .scan_prefix(OUTBOX_INDEX_KEY_PREFIX)
            .await
            .map_err(|error| OutboxRepositoryError::Unavailable(error.to_string()))?;
        let mut entries = Vec::new();
        for (key, document) in documents {
            let index = OutboxIndexCodec::decode(&document).map_err(|error| {
                OutboxRepositoryError::CorruptData(format!("outbox index {key:?}: {error}"))
            })?;
            entries.extend(index.entries.into_iter().filter(|entry| match entry.state {
                OutboxIndexState::Pending { available_at_ms } => now_ms >= available_at_ms,
                // A claim whose lease expired is work again: its owner never
                // confirmed it, and the record would otherwise sit `Claimed`
                // forever (see `OutboxState::is_dispatchable`).
                OutboxIndexState::Claimed { lease_until_ms } => now_ms >= lease_until_ms,
                OutboxIndexState::DeliveryUnknown => false,
            }));
        }
        entries.sort_by(|left, right| left.id.cmp(&right.id));
        if let Some(cursor) = after {
            entries.retain(|entry| entry.id > cursor.0);
        }
        let has_more = entries.len() > limit;
        entries.truncate(limit);
        let mut aggregates: HashMap<
            String,
            crate::scheduler::core::commit::aggregate::AggregateSnapshot,
        > = HashMap::new();
        let mut records = Vec::with_capacity(entries.len());
        for entry in entries {
            let process_id = entry.id.process_id.clone();
            let snapshot = match aggregates.get(&process_id) {
                Some(snapshot) => snapshot.clone(),
                None => {
                    let loaded = self
                        .load(&process_id)
                        .await
                        .map_err(|error| OutboxRepositoryError::Unavailable(error.to_string()))?
                        .ok_or_else(|| {
                            OutboxRepositoryError::CorruptData(format!(
                                "outbox index references missing aggregate {process_id:?}"
                            ))
                        })?;
                    aggregates.insert(process_id.clone(), loaded.snapshot.clone());
                    loaded.snapshot
                }
            };
            let record = snapshot.outbox.get(&entry.id).ok_or_else(|| {
                OutboxRepositoryError::CorruptData(format!(
                    "outbox index references missing record {:?}",
                    entry.id
                ))
            })?;
            if record.revision != entry.revision || !record.state.is_dispatchable(now_ms) {
                return Err(OutboxRepositoryError::CorruptData(format!(
                    "stale outbox index entry for {:?}",
                    entry.id
                )));
            }
            records.push(record.clone());
        }
        let next = if has_more {
            records.last().map(|last| OutboxCursor(last.id.clone()))
        } else {
            None
        };
        Ok(OutboxPage { records, next })
    }

    /// Apply one outbox lifecycle event and persist the aggregate plus both
    /// derived indexes through one guarded transaction.
    pub(crate) async fn evolve_outbox(
        &self,
        id: &OutboxId,
        expected_revision: u64,
        event: OutboxEvent,
    ) -> Result<OutboxRecord, OutboxRepositoryError> {
        let mut loaded = self
            .load(&id.process_id)
            .await
            .map_err(|error| OutboxRepositoryError::Unavailable(error.to_string()))?
            .ok_or_else(|| OutboxRepositoryError::NotFound(id.clone()))?;
        let current = loaded
            .snapshot
            .outbox
            .get(id)
            .ok_or_else(|| OutboxRepositoryError::NotFound(id.clone()))?;
        if current.revision != expected_revision {
            return Err(OutboxRepositoryError::RevisionConflict {
                id: id.clone(),
                expected: expected_revision,
                actual: current.revision,
            });
        }
        let evolved = current
            .evolve(event)
            .map_err(OutboxRepositoryError::InvalidRecord)?;
        let entries = recovery_entries(&loaded.snapshot);
        loaded.snapshot.outbox.insert(id.clone(), evolved.clone());
        match self.replace(&loaded, &loaded.snapshot, &entries).await {
            Ok(ReplaceOutcome::Committed | ReplaceOutcome::AlreadyCommitted(_)) => Ok(evolved),
            Ok(ReplaceOutcome::Conflict) => {
                Err(OutboxRepositoryError::ConcurrentModification(id.clone()))
            }
            Err(error) => Err(OutboxRepositoryError::Unavailable(error.to_string())),
        }
    }

    pub(crate) async fn replace(
        &self,
        loaded: &LoadedAggregate,
        replacement_snapshot: &crate::scheduler::core::commit::aggregate::AggregateSnapshot,
        current_recovery_entries: &std::collections::BTreeMap<String, Vec<u8>>,
    ) -> Result<ReplaceOutcome, AsyncRepositoryError> {
        let (transaction, ..) =
            build_replace_transaction(loaded, replacement_snapshot, current_recovery_entries)?;
        if self
            .store
            .atomic_batch(transaction.guards(), transaction.mutations())
            .await?
        {
            return Ok(ReplaceOutcome::Committed);
        }
        let authoritative = self.load(&loaded.snapshot.process_id).await?;
        match authoritative {
            Some(current) if current.snapshot == *replacement_snapshot => {
                Ok(ReplaceOutcome::AlreadyCommitted(Box::new(current)))
            }
            _ => Ok(ReplaceOutcome::Conflict),
        }
    }

    /// Load one aggregate with its derived indexes.
    ///
    /// The three documents are read in one `read_many` call, but a store
    /// without a native batch read serves that key by key (`KvStore::many`'s
    /// default loops `one`), so a commit landing between the reads can pair an
    /// aggregate with its neighbours from another version. The decoder catches
    /// exactly that — the indexes are derived from the aggregate — so the read
    /// is retried: it is a race with the writers this repository serves, not
    /// damage. A mismatch that survives the attempts is still surfaced.
    pub(crate) async fn load(
        &self,
        process_id: &str,
    ) -> Result<Option<LoadedAggregate>, AsyncRepositoryError> {
        let keys = vec![
            aggregate_key(process_id)?,
            recovery_index_key(process_id)?,
            outbox_index_key(process_id)?,
        ];
        for attempt in 0..LOAD_ATTEMPTS {
            let mut documents = self.store.read_many(&keys).await?.into_iter();
            let aggregate = documents.next().flatten();
            let recovery_index = documents.next().flatten();
            let outbox_index = documents.next().flatten();
            let Some(aggregate) = aggregate else {
                return Ok(None);
            };
            match decode_loaded_aggregate(process_id, aggregate, recovery_index, outbox_index) {
                Ok(loaded) => return Ok(Some(loaded)),
                Err(error) if error.is_read_race() && attempt + 1 < LOAD_ATTEMPTS => {
                    tracing::debug!(
                        process_id,
                        attempt,
                        "aggregate read raced a writer; re-reading"
                    );
                }
                Err(error) => return Err(error.into()),
            }
        }
        unreachable!("the loop returns within LOAD_ATTEMPTS")
    }
}

/// Recovery-specific production port. Recovery owns scanning and paging;
/// aggregate mutation remains on `AsyncAggregateRepository`, so workers cannot
/// bypass domain transitions or write raw documents.
pub(crate) struct AsyncRecoveryRepository {
    aggregates: Arc<AsyncAggregateRepository>,
}

impl AsyncRecoveryRepository {
    pub(crate) fn new(aggregates: Arc<AsyncAggregateRepository>) -> Self {
        Self { aggregates }
    }

    pub(crate) async fn recoverable_page(
        &self,
        after: Option<&RecoveryCursor>,
        limit: usize,
    ) -> Result<RecoveryPage, AsyncRepositoryError> {
        self.aggregates.load_recovery_page(after, limit).await
    }
}

fn insert_recovery_candidate(
    entries: &mut Vec<crate::scheduler::core::recovery::recovery_index::RecoveryIndexEntry>,
    entry: crate::scheduler::core::recovery::recovery_index::RecoveryIndexEntry,
    limit: usize,
) {
    let position = entries
        .binary_search_by(|candidate| operation_order(&candidate.operation_id, &entry.operation_id))
        .unwrap_or_else(|position| position);
    entries.insert(position, entry);
    if entries.len() > limit {
        entries.pop();
    }
}

fn authoritative_operation(
    aggregate: &LoadedAggregate,
    id: &crate::scheduler::core::model::operation::OperationId,
) -> Result<crate::scheduler::core::model::operation::OperationRecord, AsyncRepositoryError> {
    aggregate
        .snapshot
        .operations
        .get(id)
        .cloned()
        .ok_or_else(|| AsyncRepositoryError::Domain(CommitError::OperationNotFound(id.clone())))
}

#[derive(Debug)]
pub(crate) enum OperationCommitOutcome {
    Committed {
        operation: crate::scheduler::core::model::operation::OperationRecord,
        /// The snapshot this commit installed. Handed back so the driver does
        /// not have to decode the whole aggregate again to form the hop's plan.
        snapshot: crate::scheduler::core::commit::aggregate::AggregateSnapshot,
    },
    AlreadyCommitted {
        operation: crate::scheduler::core::model::operation::OperationRecord,
        /// The authoritative aggregate another writer installed.
        snapshot: crate::scheduler::core::commit::aggregate::AggregateSnapshot,
    },
    Conflict {},
}

#[derive(Debug)]
pub(crate) enum TransitionCommitOutcome {
    Committed {
        receipt: CommitReceipt,
        snapshot: crate::scheduler::core::commit::aggregate::AggregateSnapshot,
    },
    AlreadyCommitted {
        receipt: CommitReceipt,
        snapshot: crate::scheduler::core::commit::aggregate::AggregateSnapshot,
    },
    Conflict {},
}

#[derive(Debug)]
pub(crate) enum ReplaceOutcome {
    Committed,
    AlreadyCommitted(Box<LoadedAggregate>),
    Conflict,
}

#[derive(Debug)]
pub(crate) enum AsyncRepositoryError {
    Store(ActError),
    Integrity(DocumentRepositoryError),
    Conflict(String),
    Domain(CommitError),
}

impl From<ActError> for AsyncRepositoryError {
    fn from(error: ActError) -> Self {
        Self::Store(error)
    }
}

impl From<DocumentRepositoryError> for AsyncRepositoryError {
    fn from(error: DocumentRepositoryError) -> Self {
        Self::Integrity(error)
    }
}

impl std::fmt::Display for AsyncRepositoryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(error) => write!(formatter, "kernel store error: {error}"),
            Self::Integrity(error) => write!(formatter, "kernel document integrity error: {error}"),
            Self::Domain(error) => write!(formatter, "kernel domain commit error: {error}"),
            Self::Conflict(process_id) => {
                write!(
                    formatter,
                    "kernel aggregate already exists or changed: {process_id}"
                )
            }
        }
    }
}

impl std::error::Error for AsyncRepositoryError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::core::commit::aggregate::AggregateSnapshot;
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
    use crate::scheduler::core::recovery::recovery_index::{RecoveryIndex, RecoveryIndexCodec};
    use crate::scheduler::durable::document::AggregateDocumentCodec;
    use std::collections::{BTreeMap, HashMap};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// One version of an aggregate whose derived recovery index is present only
    /// when it carries the non-terminal operation: two such snapshots are two
    /// versions of the same aggregate as far as a load's consistency check is
    /// concerned.
    fn snapshot(with_operation: bool) -> AggregateSnapshot {
        let execution = ExecutionId::new("step", 0).unwrap();
        let operation_id = OperationId {
            process_id: "process".into(),
            task_id: "step".into(),
            generation: 0,
            sequence: 0,
        };
        let claim = ClaimToken {
            owner: "runtime-a".into(),
            fence: 1,
            lease_until_ms: 9_000,
            heartbeat_revision: 0,
        };
        AggregateSnapshot {
            process_id: "process".into(),
            version: AggregateVersion {
                graph: 1,
                process: 2,
                operations: u64::from(with_operation),
            },
            executions: HashMap::from([(
                execution.clone(),
                ExecutionSnapshot {
                    model_node: NodeId::parse("model-step").unwrap(),
                    parent: None,
                    predecessors: vec![],
                    state: ExecutionState::Ready,
                    revision: 1,
                },
            )]),
            variables: BTreeMap::new(),
            operations: if with_operation {
                HashMap::from([(
                    operation_id.clone(),
                    OperationRecord {
                        id: operation_id,
                        kind: OperationKind::ExecuteTask,
                        state: OperationState::Claimed(claim),
                        revision: 2,
                    },
                )])
            } else {
                HashMap::new()
            },
            outbox: HashMap::new(),
        }
    }

    /// Serves one aggregate document with a recovery index from another
    /// version for the first `torn` reads — the shape a commit landing between
    /// the three single-key reads produces — and a consistent pair after.
    /// `torn: usize::MAX` keeps serving the torn pair.
    struct TornIndexStore {
        aggregate: Vec<u8>,
        torn_index: Vec<u8>,
        torn: usize,
        reads: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl AsyncDocumentStore for TornIndexStore {
        async fn read_many(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>, ActError> {
            let read = self.reads.fetch_add(1, Ordering::SeqCst);
            let index = (read < self.torn).then(|| self.torn_index.clone());
            assert_eq!(keys.len(), 3, "aggregate + recovery index + outbox index");
            Ok(vec![Some(self.aggregate.clone()), index, None])
        }

        async fn scan_prefix(&self, _prefix: &str) -> Result<Vec<(String, Vec<u8>)>, ActError> {
            unimplemented!("the load path does not scan")
        }

        async fn atomic_batch(
            &self,
            _guards: &[DocumentGuard],
            _mutations: &[DocumentMutation],
        ) -> Result<bool, ActError> {
            unimplemented!("the load path does not write")
        }
    }

    fn torn_store(torn: usize) -> (Arc<TornIndexStore>, Arc<AtomicUsize>) {
        let reads = Arc::new(AtomicUsize::new(0));
        let store = Arc::new(TornIndexStore {
            aggregate: AggregateDocumentCodec::encode(&snapshot(false)).unwrap(),
            torn_index: RecoveryIndexCodec::encode(
                &RecoveryIndex::project(&snapshot(true)).unwrap(),
            )
            .unwrap(),
            torn,
            reads: reads.clone(),
        });
        (store, reads)
    }

    /// A load that finds the aggregate paired with a recovery index from
    /// another version re-reads instead of reporting damage — that pairing is a
    /// race with a writer, not a corrupt document.
    #[tokio::test]
    async fn load_retries_a_read_that_raced_a_writer() {
        let (store, reads) = torn_store(1);
        let repository = AsyncAggregateRepository::new(store);
        let loaded = repository
            .load("process")
            .await
            .unwrap()
            .expect("the aggregate loaded");
        assert_eq!(loaded.snapshot.version, snapshot(false).version);
        assert_eq!(reads.load(Ordering::SeqCst), 2, "the torn read was retried");
    }

    /// A store that keeps serving the mismatched pair is damage: the load
    /// surfaces it once the bounded re-reads are spent.
    #[tokio::test]
    async fn load_surfaces_a_mismatch_that_never_heals() {
        let (store, reads) = torn_store(usize::MAX);
        let repository = AsyncAggregateRepository::new(store);
        let error = repository.load("process").await.unwrap_err();
        assert!(
            format!("{error}").contains("RecoveryIndexMismatch"),
            "damage is still surfaced: {error}"
        );
        assert_eq!(reads.load(Ordering::SeqCst), LOAD_ATTEMPTS);
    }
}
