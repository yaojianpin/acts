//! Durable repository adapter for the production runtime.
//!
//! The deterministic in-memory repository remains synchronous for exhaustive
//! state-machine tests. Production storage is async and must never be hidden
//! behind `block_on`, `block_in_place`, or a dedicated thread masquerading as
//! a synchronous repository. This port is the migration boundary.

use super::commit::{
    CommitError, CommitReceipt, RecoveryCursor, RecoveryPage, apply_operation_event,
    apply_transition,
};
use super::document_store::{
    DocumentGuard, DocumentMutation, DocumentRepositoryError, LoadedAggregate,
    OUTBOX_INDEX_KEY_PREFIX, RECOVERY_INDEX_KEY_PREFIX, aggregate_key, build_create_transaction,
    build_replace_transaction, decode_loaded_aggregate, loaded_after_write, outbox_index_key,
    recovery_index_key,
};
use super::outbox::{OutboxEvent, OutboxId, OutboxRecord};
use super::outbox_index::{OutboxIndexCodec, OutboxIndexState};
use super::outbox_repository::{OutboxCursor, OutboxPage, OutboxRepositoryError};
use super::recovery_entry::{
    RECOVERY_ENTRY_KEY_PREFIX, RecoveryEntryDocument, recovery_entry_after_key,
};
use super::recovery_format::{RECOVERY_FORMAT_KEY, RecoveryFormatDocument, RecoveryFormatPhase};
use super::recovery_index::{RecoveryIndexCodec, operation_order};
use crate::ActError;
use crate::store::{KvStore, ScanOperation, ScanOptions, StoreBatchOp, StoreGuard};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

const MAX_RECOVERY_PAGE_SIZE: usize = 1_024;
const MAX_RECOVERY_MIGRATION_BATCH_SIZE: usize = 64;
/// How many times a load re-reads after finding an aggregate paired with its
/// derived indexes from another version (see [`AsyncAggregateRepository::load`]).
/// Bounded: a store that keeps serving an inconsistent triple is damage, and
/// the caller must see it.
const LOAD_ATTEMPTS: usize = 4;

#[async_trait::async_trait]
pub(crate) trait AsyncDocumentStore: Send + Sync {
    async fn read(&self, key: &str) -> Result<Option<Vec<u8>>, ActError>;

    async fn read_many(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>, ActError>;

    /// Return a stable key-ordered snapshot under `prefix`.
    async fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, ActError>;

    /// Return at most `limit` rows in key order from a half-open range.
    async fn scan_range_page(
        &self,
        prefix: &str,
        lower: Option<&str>,
        limit: usize,
    ) -> Result<Vec<(String, Vec<u8>)>, ActError>;

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
    async fn read(&self, key: &str) -> Result<Option<Vec<u8>>, ActError> {
        self.inner.one(key).await
    }

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

    async fn scan_range_page(
        &self,
        prefix: &str,
        lower: Option<&str>,
        limit: usize,
    ) -> Result<Vec<(String, Vec<u8>)>, ActError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut entries = self
            .inner
            .scan_prefix(
                prefix,
                ScanOptions::new(
                    ScanOperation::Range {
                        lower: lower.map(str::to_owned),
                        upper: prefix_upper_bound(prefix),
                    },
                    prefix.to_owned(),
                    false,
                ),
            )
            .await?;
        entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        entries.truncate(limit);
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

fn prefix_upper_bound(prefix: &str) -> Option<String> {
    let mut bytes = prefix.as_bytes().to_vec();
    while let Some(last) = bytes.pop() {
        if last != u8::MAX {
            bytes.push(last + 1);
            return String::from_utf8(bytes).ok();
        }
    }
    None
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

    /// Side-effect-free readiness check for the durable repository. A missing
    /// recovery marker is legacy V1, never an implicit V2 opt-in.
    pub(crate) async fn validate_storage(&self) -> Result<(), AsyncRepositoryError> {
        self.store.read("kernel/meta/format").await?;
        let _ = self.recovery_format_document().await?;
        Ok(())
    }

    async fn recovery_format_document(
        &self,
    ) -> Result<(Option<Vec<u8>>, RecoveryFormatDocument), AsyncRepositoryError> {
        let original = self.store.read(RECOVERY_FORMAT_KEY).await?;
        let document = match original.as_deref() {
            Some(bytes) => RecoveryFormatDocument::decode(bytes).map_err(|source| {
                AsyncRepositoryError::Domain(CommitError::CorruptAggregate(format!(
                    "recovery format document: {source}",
                )))
            })?,
            None => RecoveryFormatDocument::v1(),
        };
        Ok((original, document))
    }

    async fn transition_recovery_format(
        &self,
        original: Option<Vec<u8>>,
        replacement: RecoveryFormatDocument,
    ) -> Result<bool, AsyncRepositoryError> {
        let replacement = replacement.encode().map_err(|source| {
            AsyncRepositoryError::Domain(CommitError::CorruptAggregate(format!(
                "recovery format document: {source}",
            )))
        })?;
        let guard = DocumentGuard {
            key: RECOVERY_FORMAT_KEY.into(),
            expected: original,
        };
        let mutation = DocumentMutation::Put {
            key: RECOVERY_FORMAT_KEY.into(),
            value: replacement,
        };
        self.store
            .atomic_batch(&[guard], &[mutation])
            .await
            .map_err(Into::into)
    }

    pub(crate) async fn create(
        &self,
        snapshot: &super::AggregateSnapshot,
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

    async fn load_recovery_entry_page(
        &self,
        after: Option<&RecoveryCursor>,
        limit: usize,
    ) -> Result<RecoveryPage, AsyncRepositoryError> {
        if limit == 0 || limit > MAX_RECOVERY_PAGE_SIZE {
            return Err(AsyncRepositoryError::Domain(CommitError::InvalidPageLimit));
        }
        let lower = after.map(|cursor| recovery_entry_after_key(&cursor.0));
        let rows = self
            .store
            .scan_range_page(RECOVERY_ENTRY_KEY_PREFIX, lower.as_deref(), limit + 1)
            .await?;
        let has_more = rows.len() > limit;
        let entries = rows
            .into_iter()
            .take(limit)
            .map(|(key, value)| {
                RecoveryEntryDocument::decode(&key, &value).map_err(|source| {
                    AsyncRepositoryError::Domain(CommitError::CorruptAggregate(format!(
                        "recovery entry {key:?}: {source}",
                    )))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut aggregates = HashMap::new();
        let mut operations = Vec::with_capacity(entries.len());
        for entry in entries {
            let process_id = entry.operation_id.process_id.clone();
            if !aggregates.contains_key(&process_id) {
                let loaded = self.load(&process_id).await?.ok_or_else(|| {
                    AsyncRepositoryError::Domain(CommitError::CorruptAggregate(format!(
                        "recovery entry references missing aggregate {process_id:?}",
                    )))
                })?;
                aggregates.insert(process_id.clone(), loaded);
            }
            let aggregate = aggregates.get(&process_id).ok_or_else(|| {
                AsyncRepositoryError::Domain(CommitError::CorruptAggregate(format!(
                    "aggregate cache lost process {process_id:?}",
                )))
            })?;
            let operation = aggregate
                .snapshot
                .operations
                .get(&entry.operation_id)
                .ok_or_else(|| {
                    AsyncRepositoryError::Domain(CommitError::CorruptAggregate(format!(
                        "recovery entry references missing operation {:?}",
                        entry.operation_id,
                    )))
                })?;
            if operation.revision != entry.revision || operation.is_terminal() {
                return Err(AsyncRepositoryError::Domain(CommitError::CorruptAggregate(
                    format!(
                        "recovery entry is stale for operation {:?}",
                        entry.operation_id
                    ),
                )));
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
                let loaded = self.load(&process_id).await?.ok_or_else(|| {
                    AsyncRepositoryError::Domain(CommitError::CorruptAggregate(format!(
                        "recovery index references missing aggregate {process_id:?}",
                    )))
                })?;
                aggregates.insert(process_id.clone(), loaded);
            }
            let aggregate = aggregates.get(&process_id).ok_or_else(|| {
                AsyncRepositoryError::Domain(CommitError::CorruptAggregate(format!(
                    "aggregate cache lost process {process_id:?}",
                )))
            })?;
            let operation = aggregate
                .snapshot
                .operations
                .get(&entry.operation_id)
                .ok_or_else(|| {
                    AsyncRepositoryError::Domain(CommitError::CorruptAggregate(format!(
                        "recovery index references missing operation {:?}",
                        entry.operation_id,
                    )))
                })?;
            if operation.revision != entry.revision || operation.is_terminal() {
                return Err(AsyncRepositoryError::Domain(CommitError::CorruptAggregate(
                    format!(
                        "recovery index entry is stale for operation {:?}",
                        entry.operation_id
                    ),
                )));
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
        id: &super::OperationId,
        expected_revision: u64,
        event: super::OperationEvent,
    ) -> Result<OperationCommitOutcome, AsyncRepositoryError> {
        let loaded = self.load(&id.process_id).await?.ok_or_else(|| {
            AsyncRepositoryError::Domain(CommitError::AggregateNotFound(id.process_id.clone()))
        })?;
        let applied = apply_operation_event(&loaded.snapshot, id, expected_revision, event)
            .map_err(AsyncRepositoryError::Domain)?;
        match self.replace(&loaded, &applied.snapshot).await? {
            ReplaceOutcome::Committed(aggregate) => Ok(OperationCommitOutcome::Committed {
                operation: applied.operation,
                aggregate,
            }),
            ReplaceOutcome::AlreadyCommitted(aggregate) => {
                let operation = authoritative_operation(&aggregate, id)?;
                Ok(OperationCommitOutcome::AlreadyCommitted {
                    operation,
                    aggregate,
                })
            }
            ReplaceOutcome::Conflict(authoritative) => {
                let operation = authoritative
                    .as_ref()
                    .and_then(|aggregate| aggregate.snapshot.operations.get(id))
                    .cloned();
                Ok(OperationCommitOutcome::Conflict {
                    operation,
                    authoritative,
                })
            }
        }
    }

    /// Apply a pure domain transition and persist the aggregate plus both
    /// derived indexes through one guarded transaction.
    pub(crate) async fn commit_transition(
        &self,
        plan: &super::TransitionPlan,
    ) -> Result<TransitionCommitOutcome, AsyncRepositoryError> {
        let loaded = self.load(&plan.process_id).await?.ok_or_else(|| {
            AsyncRepositoryError::Domain(CommitError::AggregateNotFound(plan.process_id.clone()))
        })?;
        let applied =
            apply_transition(&loaded.snapshot, plan).map_err(AsyncRepositoryError::Domain)?;
        match self.replace(&loaded, &applied.snapshot).await? {
            ReplaceOutcome::Committed(aggregate) => Ok(TransitionCommitOutcome::Committed {
                receipt: applied.receipt,
                aggregate,
            }),
            ReplaceOutcome::AlreadyCommitted(aggregate) => {
                Ok(TransitionCommitOutcome::AlreadyCommitted {
                    receipt: applied.receipt,
                    aggregate,
                })
            }
            ReplaceOutcome::Conflict(authoritative) => {
                Ok(TransitionCommitOutcome::Conflict { authoritative })
            }
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
            entries.extend(index.entries.into_iter().filter(|entry| {
                matches!(entry.state, OutboxIndexState::Pending { available_at_ms } if now_ms >= available_at_ms)
            }));
        }
        entries.sort_by(|left, right| left.id.cmp(&right.id));
        if let Some(cursor) = after {
            entries.retain(|entry| entry.id > cursor.0);
        }
        let has_more = entries.len() > limit;
        entries.truncate(limit);
        let mut aggregates: HashMap<String, super::AggregateSnapshot> = HashMap::new();
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
        let loaded = self
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
        let mut replacement = loaded.snapshot.clone();
        replacement.outbox.insert(id.clone(), evolved.clone());
        match self.replace(&loaded, &replacement).await {
            Ok(ReplaceOutcome::Committed(_) | ReplaceOutcome::AlreadyCommitted(_)) => Ok(evolved),
            Ok(ReplaceOutcome::Conflict(_)) => {
                Err(OutboxRepositoryError::ConcurrentModification(id.clone()))
            }
            Err(error) => Err(OutboxRepositoryError::Unavailable(error.to_string())),
        }
    }

    pub(crate) async fn replace(
        &self,
        loaded: &LoadedAggregate,
        replacement_snapshot: &super::AggregateSnapshot,
    ) -> Result<ReplaceOutcome, AsyncRepositoryError> {
        let (transaction, document, recovery_index, outbox_index) =
            build_replace_transaction(loaded, replacement_snapshot)?;
        if self
            .store
            .atomic_batch(transaction.guards(), transaction.mutations())
            .await?
        {
            return Ok(ReplaceOutcome::Committed(loaded_after_write(
                replacement_snapshot,
                document,
                recovery_index,
                outbox_index,
            )));
        }
        let authoritative = self.load(&loaded.snapshot.process_id).await?;
        match authoritative {
            Some(current) if current.snapshot == *replacement_snapshot => {
                Ok(ReplaceOutcome::AlreadyCommitted(current))
            }
            current => Ok(ReplaceOutcome::Conflict(current)),
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

    pub(crate) async fn validate_storage(&self) -> Result<(), AsyncRepositoryError> {
        self.aggregates.validate_storage().await
    }

    pub(crate) async fn migration_state(
        &self,
    ) -> Result<RecoveryMigrationState, AsyncRepositoryError> {
        let (_, format) = self.aggregates.recovery_format_document().await?;
        Ok(match format.phase {
            RecoveryFormatPhase::V1 => RecoveryMigrationState::V1,
            RecoveryFormatPhase::Migrating => RecoveryMigrationState::Migrating,
            RecoveryFormatPhase::V2 => RecoveryMigrationState::V2,
        })
    }

    pub(crate) async fn page_for_active_format(
        &self,
        after: Option<&RecoveryCursor>,
        limit: usize,
    ) -> Result<RecoveryPage, AsyncRepositoryError> {
        let (_, format) = self.aggregates.recovery_format_document().await?;
        match format.phase {
            RecoveryFormatPhase::V1 => self.aggregates.load_recovery_page(after, limit).await,
            RecoveryFormatPhase::V2 => self.aggregates.load_recovery_entry_page(after, limit).await,
            RecoveryFormatPhase::Migrating => {
                Err(AsyncRepositoryError::Domain(CommitError::CorruptAggregate(
                    "recovery index migration is incomplete; recovery workers are disabled".into(),
                )))
            }
        }
    }

    pub(crate) async fn begin_v2_migration(&self) -> Result<bool, AsyncRepositoryError> {
        let (original, format) = self.aggregates.recovery_format_document().await?;
        match format.phase {
            RecoveryFormatPhase::V1 => {
                self.aggregates
                    .transition_recovery_format(original, RecoveryFormatDocument::migrating(None))
                    .await
            }
            RecoveryFormatPhase::Migrating | RecoveryFormatPhase::V2 => Ok(false),
        }
    }

    pub(crate) async fn migrate_v2_batch(
        &self,
        limit: usize,
    ) -> Result<RecoveryMigrationBatch, AsyncRepositoryError> {
        if limit == 0 || limit > MAX_RECOVERY_MIGRATION_BATCH_SIZE {
            return Err(AsyncRepositoryError::Domain(CommitError::InvalidPageLimit));
        }
        let (format_bytes, format) = self.aggregates.recovery_format_document().await?;
        if format.phase != RecoveryFormatPhase::Migrating {
            return Err(AsyncRepositoryError::Domain(CommitError::CorruptAggregate(
                "recovery V2 rebuild requires Migrating format phase".into(),
            )));
        }
        let lower = format.cursor.as_deref().map(exclusive_key_after);
        let rows = self
            .aggregates
            .store
            .scan_range_page(RECOVERY_INDEX_KEY_PREFIX, lower.as_deref(), limit)
            .await?;
        if rows.is_empty() {
            return Ok(RecoveryMigrationBatch {
                rebuilt_process_indexes: 0,
                complete: true,
                committed: true,
            });
        }
        let mut guards = vec![DocumentGuard {
            key: RECOVERY_FORMAT_KEY.into(),
            expected: format_bytes,
        }];
        let mut mutations = Vec::new();
        for (index_key, index_bytes) in &rows {
            let index = RecoveryIndexCodec::decode(index_bytes).map_err(|source| {
                AsyncRepositoryError::Domain(CommitError::CorruptAggregate(format!(
                    "recovery index {index_key:?}: {source}",
                )))
            })?;
            let loaded = self
                .aggregates
                .load(&index.process_id)
                .await?
                .ok_or_else(|| {
                    AsyncRepositoryError::Domain(CommitError::CorruptAggregate(format!(
                        "recovery index references missing aggregate {:?}",
                        index.process_id,
                    )))
                })?;
            guards.push(DocumentGuard {
                key: aggregate_key(&index.process_id)?,
                expected: Some(loaded.original_document().to_vec()),
            });
            guards.push(DocumentGuard {
                key: index_key.clone(),
                expected: Some(index_bytes.clone()),
            });
            let projected = RecoveryEntryDocument::project(&loaded.snapshot);
            let keys = projected.keys().cloned().collect::<Vec<_>>();
            let existing = self.aggregates.store.read_many(&keys).await?;
            for ((entry_key, expected), current) in projected.into_iter().zip(existing) {
                match current {
                    Some(current) if current == expected => {
                        guards.push(DocumentGuard {
                            key: entry_key,
                            expected: Some(current),
                        });
                    }
                    Some(_) => {
                        return Err(AsyncRepositoryError::Domain(CommitError::CorruptAggregate(
                            format!(
                                "recovery V2 entry conflicts with aggregate projection at {entry_key:?}",
                            ),
                        )));
                    }
                    None => {
                        guards.push(DocumentGuard {
                            key: entry_key.clone(),
                            expected: None,
                        });
                        mutations.push(DocumentMutation::Put {
                            key: entry_key,
                            value: expected,
                        });
                    }
                }
            }
        }
        let last_key = rows.last().map(|(key, _)| key.clone()).ok_or_else(|| {
            AsyncRepositoryError::Domain(CommitError::CorruptAggregate(
                "recovery migration batch unexpectedly lost its cursor".into(),
            ))
        })?;
        let next_format = RecoveryFormatDocument::migrating(Some(last_key));
        mutations.push(DocumentMutation::Put {
            key: RECOVERY_FORMAT_KEY.into(),
            value: next_format.encode().map_err(|source| {
                AsyncRepositoryError::Domain(CommitError::CorruptAggregate(format!(
                    "recovery format document: {source}",
                )))
            })?,
        });
        let committed = self
            .aggregates
            .store
            .atomic_batch(&guards, &mutations)
            .await?;
        Ok(RecoveryMigrationBatch {
            rebuilt_process_indexes: rows.len(),
            complete: rows.len() < limit,
            committed,
        })
    }

    pub(crate) async fn finish_v2_migration(&self) -> Result<bool, AsyncRepositoryError> {
        let (original, format) = self.aggregates.recovery_format_document().await?;
        if format.phase != RecoveryFormatPhase::Migrating {
            return Ok(false);
        }
        let lower = format.cursor.as_deref().map(exclusive_key_after);
        if !self
            .aggregates
            .store
            .scan_range_page(RECOVERY_INDEX_KEY_PREFIX, lower.as_deref(), 1)
            .await?
            .is_empty()
        {
            return Ok(false);
        }
        self.verify_v2_projection().await?;
        self.aggregates
            .transition_recovery_format(original, RecoveryFormatDocument::v2())
            .await
    }

    async fn verify_v2_projection(&self) -> Result<(), AsyncRepositoryError> {
        let mut expected = BTreeMap::new();
        for (index_key, index_bytes) in self
            .aggregates
            .store
            .scan_prefix(RECOVERY_INDEX_KEY_PREFIX)
            .await?
        {
            let index = RecoveryIndexCodec::decode(&index_bytes).map_err(|source| {
                AsyncRepositoryError::Domain(CommitError::CorruptAggregate(format!(
                    "recovery index {index_key:?}: {source}",
                )))
            })?;
            let loaded = self
                .aggregates
                .load(&index.process_id)
                .await?
                .ok_or_else(|| {
                    AsyncRepositoryError::Domain(CommitError::CorruptAggregate(format!(
                        "recovery index references missing aggregate {:?}",
                        index.process_id,
                    )))
                })?;
            for (key, value) in RecoveryEntryDocument::project(&loaded.snapshot) {
                if expected.insert(key.clone(), value).is_some() {
                    return Err(AsyncRepositoryError::Domain(CommitError::CorruptAggregate(
                        format!("duplicate recovery V2 projection key {key:?}",),
                    )));
                }
            }
        }
        for (key, value) in self
            .aggregates
            .store
            .scan_prefix(RECOVERY_ENTRY_KEY_PREFIX)
            .await?
        {
            let projected = expected.remove(&key).ok_or_else(|| {
                AsyncRepositoryError::Domain(CommitError::CorruptAggregate(format!(
                    "unexpected recovery V2 entry {key:?}",
                )))
            })?;
            if projected != value {
                return Err(AsyncRepositoryError::Domain(CommitError::CorruptAggregate(
                    format!("recovery V2 entry differs from aggregate projection at {key:?}",),
                )));
            }
        }
        if let Some((key, _)) = expected.into_iter().next() {
            return Err(AsyncRepositoryError::Domain(CommitError::CorruptAggregate(
                format!("missing recovery V2 entry {key:?}",),
            )));
        }
        Ok(())
    }

    pub(crate) async fn recovery_entry_page(
        &self,
        after: Option<&RecoveryCursor>,
        limit: usize,
    ) -> Result<RecoveryPage, AsyncRepositoryError> {
        self.aggregates.load_recovery_entry_page(after, limit).await
    }

    pub(crate) async fn recoverable_page(
        &self,
        after: Option<&RecoveryCursor>,
        limit: usize,
    ) -> Result<RecoveryPage, AsyncRepositoryError> {
        self.aggregates.load_recovery_page(after, limit).await
    }

    pub(crate) async fn commit_operation_event(
        &self,
        id: &super::OperationId,
        expected_revision: u64,
        event: super::OperationEvent,
    ) -> Result<OperationCommitOutcome, AsyncRepositoryError> {
        self.aggregates
            .commit_operation_event(id, expected_revision, event)
            .await
    }
}

fn exclusive_key_after(key: &str) -> String {
    let mut lower = String::with_capacity(key.len() + 1);
    lower.push_str(key);
    lower.push('\0');
    lower
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RecoveryMigrationState {
    V1,
    Migrating,
    V2,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RecoveryMigrationBatch {
    pub(crate) rebuilt_process_indexes: usize,
    pub(crate) complete: bool,
    pub(crate) committed: bool,
}

fn insert_recovery_candidate(
    entries: &mut Vec<super::recovery_index::RecoveryIndexEntry>,
    entry: super::recovery_index::RecoveryIndexEntry,
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
    id: &super::OperationId,
) -> Result<super::OperationRecord, AsyncRepositoryError> {
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
        operation: super::OperationRecord,
        aggregate: LoadedAggregate,
    },
    AlreadyCommitted {
        operation: super::OperationRecord,
        aggregate: LoadedAggregate,
    },
    Conflict {
        operation: Option<super::OperationRecord>,
        authoritative: Option<LoadedAggregate>,
    },
}

#[derive(Debug)]
pub(crate) enum TransitionCommitOutcome {
    Committed {
        receipt: CommitReceipt,
        aggregate: LoadedAggregate,
    },
    AlreadyCommitted {
        receipt: CommitReceipt,
        aggregate: LoadedAggregate,
    },
    Conflict {
        authoritative: Option<LoadedAggregate>,
    },
}

#[derive(Debug)]
pub(crate) enum ReplaceOutcome {
    Committed(LoadedAggregate),
    AlreadyCommitted(LoadedAggregate),
    Conflict(Option<LoadedAggregate>),
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
    use super::super::document::AggregateDocumentCodec;
    use super::super::recovery_index::{RecoveryIndex, RecoveryIndexCodec};
    use super::*;
    use crate::kernel::{
        AggregateSnapshot, AggregateVersion, ClaimToken, ExecutionId, ExecutionSnapshot,
        ExecutionState, NodeId, OperationId, OperationKind, OperationRecord, OperationState,
    };
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
        async fn read(&self, _key: &str) -> Result<Option<Vec<u8>>, ActError> {
            unimplemented!("the load path reads through read_many")
        }

        async fn read_many(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>, ActError> {
            let read = self.reads.fetch_add(1, Ordering::SeqCst);
            let index = (read < self.torn).then(|| self.torn_index.clone());
            assert_eq!(keys.len(), 3, "aggregate + recovery index + outbox index");
            Ok(vec![Some(self.aggregate.clone()), index, None])
        }

        async fn scan_prefix(&self, _prefix: &str) -> Result<Vec<(String, Vec<u8>)>, ActError> {
            unimplemented!("the load path does not scan")
        }

        async fn scan_range_page(
            &self,
            _prefix: &str,
            _lower: Option<&str>,
            _limit: usize,
        ) -> Result<Vec<(String, Vec<u8>)>, ActError> {
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
