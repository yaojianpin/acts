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
                CommitError, CommitReceipt, ReadyCursor, ReadyPage, RecoveryCursor, RecoveryPage,
                advance_operation_event, advance_transition,
            },
            outbox::{
                outbox_index::{OutboxIndexCodec, OutboxIndexState},
                outbox_repository::{OutboxCursor, OutboxPage, OutboxRepositoryError},
                records::{OutboxEvent, OutboxId, OutboxRecord},
            },
            ready::{ReadyIndex, ReadyIndexCodec},
            recovery::recovery_index::RecoveryIndexCodec,
        },
        durable::document_store::{
            DocumentGuard, DocumentMutation, DocumentRepositoryError, LoadedProcess,
            OUTBOX_INDEX_KEY_PREFIX, PROCESS_KEY_PREFIX, READY_INDEX_KEY_PREFIX,
            RECOVERY_INDEX_KEY_PREFIX, build_create_transaction, build_repair_transaction,
            build_replace_transaction, decode_loaded_process, decode_process_key,
            loaded_after_write, outbox_index_key, process_key, ready_index_key, recovery_entries,
            recovery_index_document, recovery_index_key,
        },
    },
    store::{KvStore, ScanOperation, ScanOptions, StoreBatchOp, StoreGuard},
};

use std::collections::HashMap;
use std::sync::Arc;

const MAX_RECOVERY_PAGE_SIZE: usize = 1_024;
/// How many times a load re-reads after finding an aggregate paired with its
/// derived indexes from another version (see [`AsyncProcessRepository::load`]).
/// Bounded: a store that keeps serving an inconsistent triple is damage, and
/// the caller must see it.
const LOAD_ATTEMPTS: usize = 4;

#[async_trait::async_trait]
pub(crate) trait AsyncDocumentStore: Send + Sync {
    async fn read_many(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>, ActError>;

    /// Return a stable key-ordered snapshot under `prefix`.
    async fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, ActError>;

    /// Return the key-ordered entries under `prefix` inside the half-open
    /// range `[lower, upper)`. An absent bound is unrestricted on that side.
    /// Paging uses this so a page resumes at its cursor instead of rescanning
    /// the documents before it.
    async fn scan_range(
        &self,
        prefix: &str,
        lower: Option<&str>,
        upper: Option<&str>,
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

    async fn scan_range(
        &self,
        prefix: &str,
        lower: Option<&str>,
        upper: Option<&str>,
    ) -> Result<Vec<(String, Vec<u8>)>, ActError> {
        let mut entries = self
            .inner
            .scan_prefix(
                prefix,
                ScanOptions::new(
                    ScanOperation::Range {
                        lower: lower.map(str::to_owned),
                        upper: upper.map(str::to_owned),
                    },
                    prefix.to_owned(),
                    false,
                ),
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
pub(crate) struct AsyncProcessRepository {
    store: Arc<dyn AsyncDocumentStore>,
}

impl AsyncProcessRepository {
    pub(crate) fn new(store: Arc<dyn AsyncDocumentStore>) -> Self {
        Self { store }
    }

    pub(crate) async fn create(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        now_ms: i64,
        poll_ms: i64,
    ) -> Result<LoadedProcess, AsyncRepositoryError> {
        let (transaction, document, recovery_index, outbox_index, ready_index) =
            build_create_transaction(snapshot, now_ms, poll_ms)?;
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
            ready_index,
        ))
    }

    /// Read a stable page from the compact recovery projection, then validate
    /// every selected row against its authoritative aggregate before exposing it.
    ///
    /// The cursor is positional: a page starts at the recovery document it
    /// stopped in and continues after the last index entry it examined. No
    /// document is scanned twice for one pagination walk, so the cost of a
    /// page is bounded by `limit` index entries plus the aggregates it loads —
    /// never by the whole recovery index.
    async fn load_recovery_page(
        &self,
        after: Option<&RecoveryCursor>,
        limit: usize,
    ) -> Result<RecoveryPage, AsyncRepositoryError> {
        if limit == 0 || limit > MAX_RECOVERY_PAGE_SIZE {
            return Err(AsyncRepositoryError::Domain(CommitError::InvalidPageLimit));
        }
        let lower = after.map(|cursor| cursor.process_key.as_str());
        let documents = self
            .store
            .scan_range(RECOVERY_INDEX_KEY_PREFIX, lower, None)
            .await?;
        let mut candidates = Vec::with_capacity(limit + 1);
        'documents: for (key, bytes) in documents {
            let index = RecoveryIndexCodec::decode(&bytes).map_err(|source| {
                AsyncRepositoryError::Domain(CommitError::CorruptProcess(format!(
                    "recovery index {key:?}: {source}",
                )))
            })?;
            for entry in index.entries {
                if let Some(cursor) = after
                    && key == cursor.process_key
                    && let Some(operation) = &cursor.operation
                    && !crate::scheduler::core::recovery::recovery_index::operation_order(
                        &entry.operation_id,
                        operation,
                    )
                    .is_gt()
                {
                    continue;
                }
                candidates.push((key.clone(), entry));
                if candidates.len() > limit {
                    break 'documents;
                }
            }
        }
        let has_more = candidates.len() > limit;
        candidates.truncate(limit);
        let next = has_more.then(|| {
            let (process_key, entry) = candidates.last().expect("has_more implies a candidate");
            RecoveryCursor {
                process_key: process_key.clone(),
                operation: Some(entry.operation_id.clone()),
            }
        });

        let mut processes = HashMap::new();
        let mut operations = Vec::with_capacity(candidates.len());
        for (_, entry) in candidates {
            let process_id = entry.operation_id.process_id.clone();
            if !processes.contains_key(&process_id) {
                // The aggregates are loaded at a later instant than the index
                // documents the entries came from, so the two can disagree: a
                // run swept (or a commit landing) in between leaves the index
                // the older view. That is the scan being stale, not damage —
                // and the projection is written atomically with its aggregate,
                // so a disagreement can only be this race. The entry is
                // skipped; the next pass re-reads the index.
                match self.load(&process_id).await? {
                    Some(loaded) => {
                        processes.insert(process_id.clone(), loaded);
                    }
                    None => continue,
                }
            }
            let process = processes.get(&process_id).ok_or_else(|| {
                AsyncRepositoryError::Domain(CommitError::CorruptProcess(format!(
                    "aggregate cache lost process {process_id:?}",
                )))
            })?;
            let Some(operation) = process.snapshot.operations.get(&entry.operation_id) else {
                // The operation left the aggregate (it settled, and a terminal
                // operation is dropped from the index in the same commit) after
                // the index doc was scanned: stale, not damage.
                continue;
            };
            if operation.revision != entry.revision || operation.is_retired() {
                // Same: the operation moved on after the index was scanned.
                continue;
            }
            operations.push(operation.clone());
        }
        Ok(RecoveryPage { operations, next })
    }

    /// Apply one operation lifecycle event and atomically persist the
    /// operation, aggregate operation version and all derived indexes.
    pub(crate) async fn commit_operation_event(
        &self,
        id: &crate::scheduler::core::model::operation::OperationId,
        expected_revision: u64,
        event: crate::scheduler::core::model::operation::OperationEvent,
        now_ms: i64,
        poll_ms: i64,
    ) -> Result<OperationCommitOutcome, AsyncRepositoryError> {
        let mut loaded = self.load(&id.process_id).await?.ok_or_else(|| {
            AsyncRepositoryError::Domain(CommitError::ProcessNotFound(id.process_id.clone()))
        })?;
        // The advance runs on the loaded aggregate itself: this repository owns
        // it and drops it on failure (the caller reports and recovery
        // re-derives the operation), so the whole-aggregate clone is not needed.
        let entries = recovery_entries(&loaded.snapshot);
        let operation = advance_operation_event(&mut loaded.snapshot, id, expected_revision, event)
            .map_err(AsyncRepositoryError::Domain)?;
        let ready = self.ready_document(&loaded, now_ms, poll_ms, &HashMap::new())?;
        let outcome = self
            .replace(&loaded, &loaded.snapshot, &entries, ready.as_deref())
            .await?;
        match outcome {
            ReplaceOutcome::Committed => Ok(OperationCommitOutcome::Committed {
                operation,
                snapshot: loaded.snapshot,
            }),
            ReplaceOutcome::AlreadyCommitted(process) => {
                let operation = authoritative_operation(&process, id)?;
                Ok(OperationCommitOutcome::AlreadyCommitted {
                    operation,
                    snapshot: process.snapshot.clone(),
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
        now_ms: i64,
        poll_ms: i64,
    ) -> Result<TransitionCommitOutcome, AsyncRepositoryError> {
        let mut loaded = self.load(&plan.process_id).await?.ok_or_else(|| {
            AsyncRepositoryError::Domain(CommitError::ProcessNotFound(plan.process_id.clone()))
        })?;
        // In place, like the operation event above.
        let entries = recovery_entries(&loaded.snapshot);
        let ready_at = plan
            .operations
            .iter()
            .filter_map(|intent| {
                intent
                    .ready_at_ms
                    .map(|ready_at| (intent.id.clone(), ready_at))
            })
            .collect::<HashMap<_, _>>();
        let receipt =
            advance_transition(&mut loaded.snapshot, plan).map_err(AsyncRepositoryError::Domain)?;
        let ready = self.ready_document(&loaded, now_ms, poll_ms, &ready_at)?;
        let outcome = self
            .replace(&loaded, &loaded.snapshot, &entries, ready.as_deref())
            .await?;
        match outcome {
            ReplaceOutcome::Committed => Ok(TransitionCommitOutcome::Committed {
                receipt,
                snapshot: loaded.snapshot,
            }),
            ReplaceOutcome::AlreadyCommitted(process) => {
                Ok(TransitionCommitOutcome::AlreadyCommitted {
                    receipt,
                    snapshot: process.snapshot.clone(),
                })
            }
            ReplaceOutcome::Conflict => Ok(TransitionCommitOutcome::Conflict {}),
        }
    }

    /// Read the due records of the durable ready index, in
    /// `(process document, ready_at, operation)` order. The cursor is
    /// positional, so each call resumes after the last examined entry and a
    /// paged walk never rescans a document. `observed` and
    /// `oldest_ready_at_ms` describe every entry the page walked, due or not,
    /// for the backlog metrics.
    pub(crate) async fn due_ready_page(
        &self,
        after: Option<&ReadyCursor>,
        now_ms: i64,
        limit: usize,
    ) -> Result<ReadyPage, AsyncRepositoryError> {
        if limit == 0 || limit > MAX_RECOVERY_PAGE_SIZE {
            return Err(AsyncRepositoryError::Domain(CommitError::InvalidPageLimit));
        }
        let lower = after.map(|cursor| cursor.process_key.as_str());
        let documents = self
            .store
            .scan_range(READY_INDEX_KEY_PREFIX, lower, None)
            .await?;
        let mut observed = 0usize;
        let mut oldest: Option<i64> = None;
        let mut due: Vec<(
            String,
            crate::scheduler::core::model::operation::OperationId,
            i64,
        )> = Vec::with_capacity(limit + 1);
        'documents: for (key, bytes) in documents {
            let index = ReadyIndexCodec::decode(&bytes).map_err(|source| {
                AsyncRepositoryError::Domain(CommitError::CorruptProcess(format!(
                    "ready index {key:?}: {source}"
                )))
            })?;
            for entry in index.entries {
                if let Some(cursor) = after
                    && key == cursor.process_key
                    && (entry.ready_at_ms < cursor.ready_at_ms
                        || (entry.ready_at_ms == cursor.ready_at_ms
                            && !crate::scheduler::core::recovery::recovery_index::operation_order(
                                &entry.operation_id,
                                &cursor.operation,
                            )
                            .is_gt()))
                {
                    continue;
                }
                observed += 1;
                oldest =
                    Some(oldest.map_or(entry.ready_at_ms, |value| value.min(entry.ready_at_ms)));
                if entry.ready_at_ms <= now_ms {
                    due.push((key.clone(), entry.operation_id, entry.ready_at_ms));
                    if due.len() > limit {
                        break 'documents;
                    }
                }
            }
        }
        let has_more = due.len() > limit;
        due.truncate(limit);
        let next = has_more.then(|| {
            let (process_key, operation, ready_at_ms) =
                due.last().expect("has_more implies a due entry");
            ReadyCursor {
                process_key: process_key.clone(),
                ready_at_ms: *ready_at_ms,
                operation: operation.clone(),
            }
        });
        Ok(ReadyPage {
            operations: due.into_iter().map(|(_, operation, _)| operation).collect(),
            next,
            observed,
            oldest_ready_at_ms: oldest,
        })
    }

    /// Set one ready record's due time, if it is still present.
    ///
    /// This never adds a record: a missing entry means the operation was
    /// claimed, completed or removed, and re-adding it would resurrect work.
    /// The write is guarded by the ready document's bytes, so a concurrent
    /// aggregate commit wins and this call simply retries against the new
    /// document (or finds the entry gone). `until_ms` may move in either
    /// direction: admission makes a parked record due now, an idle round
    /// defers it.
    pub(crate) async fn set_ready_at(
        &self,
        id: &crate::scheduler::core::model::operation::OperationId,
        until_ms: i64,
    ) -> Result<bool, AsyncRepositoryError> {
        let key = ready_index_key(&id.process_id)?;
        for _ in 0..8 {
            let Some(bytes) = self
                .store
                .read_many(std::slice::from_ref(&key))
                .await?
                .pop()
                .flatten()
            else {
                return Ok(false);
            };
            let mut index = ReadyIndexCodec::decode(&bytes).map_err(|source| {
                AsyncRepositoryError::Domain(CommitError::CorruptProcess(format!(
                    "ready index {key:?}: {source}"
                )))
            })?;
            let Some(entry) = index
                .entries
                .iter_mut()
                .find(|entry| &entry.operation_id == id)
            else {
                return Ok(false);
            };
            if entry.ready_at_ms == until_ms {
                return Ok(false);
            }
            entry.ready_at_ms = until_ms;
            index.entries.sort_by(|left, right| {
                left.ready_at_ms.cmp(&right.ready_at_ms).then_with(|| {
                    crate::scheduler::core::recovery::recovery_index::operation_order(
                        &left.operation_id,
                        &right.operation_id,
                    )
                })
            });
            let replacement = ReadyIndexCodec::encode(&index).map_err(|source| {
                AsyncRepositoryError::Domain(CommitError::CorruptProcess(format!(
                    "ready index {key:?}: {source}"
                )))
            })?;
            let batch = [DocumentMutation::Put {
                key: key.clone(),
                value: replacement,
            }];
            let guards = [DocumentGuard {
                key: key.clone(),
                expected: Some(bytes),
            }];
            if self.store.atomic_batch(&guards, &batch).await? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Rebuild one process's derived indexes from its authoritative aggregate:
    /// the normalized recovery projection and a ready record for every
    /// schedulable operation. Called once at boot; a normal commit keeps both
    /// exact, so this is the repair path for an older database or a damaged
    /// index. Existing ready times are preserved, so a deferred timer is not
    /// made hot by a restart.
    pub(crate) async fn repair_ready(
        &self,
        process_id: &str,
        now_ms: i64,
        poll_ms: i64,
    ) -> Result<bool, AsyncRepositoryError> {
        for _ in 0..LOAD_ATTEMPTS {
            let Some(loaded) = self.load(process_id).await? else {
                return Ok(false);
            };
            let recovery = recovery_index_document(&loaded.snapshot)?;
            let previous = loaded
                .original_ready_index()
                .and_then(|bytes| ReadyIndexCodec::decode(bytes).ok());
            let ready = ReadyIndex::reconcile(
                previous.as_ref(),
                &loaded.snapshot,
                now_ms,
                poll_ms,
                &HashMap::new(),
            )
            .map(|index| {
                ReadyIndexCodec::encode(&index).map_err(|source| {
                    AsyncRepositoryError::Domain(CommitError::CorruptProcess(format!(
                        "ready index for {process_id:?}: {source}"
                    )))
                })
            })
            .transpose()?;
            let Some(transaction) =
                build_repair_transaction(&loaded, recovery.as_deref(), ready.as_deref())?
            else {
                return Ok(false);
            };
            if self
                .store
                .atomic_batch(transaction.guards(), transaction.mutations())
                .await?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Repair every aggregate's derived indexes. One boot pass: the scan is
    /// linear in the aggregate set, never in the number of ticks.
    pub(crate) async fn repair_all_ready(
        &self,
        now_ms: i64,
        poll_ms: i64,
    ) -> Result<usize, AsyncRepositoryError> {
        let mut repaired = 0usize;
        for (key, _) in self.store.scan_prefix(PROCESS_KEY_PREFIX).await? {
            let process_id = decode_process_key(&key)?;
            if self.repair_ready(&process_id, now_ms, poll_ms).await? {
                repaired += 1;
            }
        }
        Ok(repaired)
    }

    fn ready_document(
        &self,
        loaded: &LoadedProcess,
        now_ms: i64,
        poll_ms: i64,
        ready_at: &HashMap<crate::scheduler::core::model::operation::OperationId, i64>,
    ) -> Result<Option<Vec<u8>>, AsyncRepositoryError> {
        let previous = loaded
            .original_ready_index()
            .map(ReadyIndexCodec::decode)
            .transpose()
            .map_err(|source| {
                AsyncRepositoryError::Domain(CommitError::CorruptProcess(format!(
                    "ready index for {:?}: {source}",
                    loaded.snapshot.process_id
                )))
            })?;
        ReadyIndex::reconcile(
            previous.as_ref(),
            &loaded.snapshot,
            now_ms,
            poll_ms,
            ready_at,
        )
        .map(|index| {
            ReadyIndexCodec::encode(&index).map_err(|source| {
                AsyncRepositoryError::Domain(CommitError::CorruptProcess(format!(
                    "ready index for {:?}: {source}",
                    loaded.snapshot.process_id
                )))
            })
        })
        .transpose()
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
        let mut processes: HashMap<
            String,
            crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        > = HashMap::new();
        let mut records = Vec::with_capacity(entries.len());
        for entry in entries {
            let process_id = entry.id.process_id.clone();
            let snapshot = match processes.get(&process_id) {
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
                    processes.insert(process_id.clone(), loaded.snapshot.clone());
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
        let ready = loaded.original_ready_index().map(ToOwned::to_owned);
        match self
            .replace(&loaded, &loaded.snapshot, &entries, ready.as_deref())
            .await
        {
            Ok(ReplaceOutcome::Committed | ReplaceOutcome::AlreadyCommitted(_)) => Ok(evolved),
            Ok(ReplaceOutcome::Conflict) => {
                Err(OutboxRepositoryError::ConcurrentModification(id.clone()))
            }
            Err(error) => Err(OutboxRepositoryError::Unavailable(error.to_string())),
        }
    }

    pub(crate) async fn replace(
        &self,
        loaded: &LoadedProcess,
        replacement_snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        current_recovery_entries: &std::collections::BTreeMap<String, Vec<u8>>,
        replacement_ready: Option<&[u8]>,
    ) -> Result<ReplaceOutcome, AsyncRepositoryError> {
        let (transaction, ..) = build_replace_transaction(
            loaded,
            replacement_snapshot,
            current_recovery_entries,
            replacement_ready,
        )?;
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
    ) -> Result<Option<LoadedProcess>, AsyncRepositoryError> {
        let keys = vec![
            process_key(process_id)?,
            recovery_index_key(process_id)?,
            outbox_index_key(process_id)?,
            ready_index_key(process_id)?,
        ];
        for attempt in 0..LOAD_ATTEMPTS {
            let mut documents = self.store.read_many(&keys).await?.into_iter();
            let process = documents.next().flatten();
            let recovery_index = documents.next().flatten();
            let outbox_index = documents.next().flatten();
            let ready_index = documents.next().flatten();
            let Some(process) = process else {
                return Ok(None);
            };
            match decode_loaded_process(
                process_id,
                process,
                recovery_index,
                outbox_index,
                ready_index,
            ) {
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
/// aggregate mutation remains on `AsyncProcessRepository`, so workers cannot
/// bypass domain transitions or write raw documents.
pub(crate) struct AsyncRecoveryRepository {
    processes: Arc<AsyncProcessRepository>,
}

impl AsyncRecoveryRepository {
    pub(crate) fn new(processes: Arc<AsyncProcessRepository>) -> Self {
        Self { processes }
    }

    pub(crate) async fn recoverable_page(
        &self,
        after: Option<&RecoveryCursor>,
        limit: usize,
    ) -> Result<RecoveryPage, AsyncRepositoryError> {
        self.processes.load_recovery_page(after, limit).await
    }
}

fn authoritative_operation(
    process: &LoadedProcess,
    id: &crate::scheduler::core::model::operation::OperationId,
) -> Result<crate::scheduler::core::model::operation::OperationRecord, AsyncRepositoryError> {
    process
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
        /// not have to decode the whole aggregate again to form the operation's plan.
        snapshot: crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    },
    AlreadyCommitted {
        operation: crate::scheduler::core::model::operation::OperationRecord,
        /// The authoritative aggregate another writer installed.
        snapshot: crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    },
    Conflict {},
}

#[derive(Debug)]
pub(crate) enum TransitionCommitOutcome {
    Committed {
        receipt: CommitReceipt,
        snapshot: crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    },
    AlreadyCommitted {
        receipt: CommitReceipt,
        snapshot: crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    },
    Conflict {},
}

#[derive(Debug)]
pub(crate) enum ReplaceOutcome {
    Committed,
    AlreadyCommitted(Box<LoadedProcess>),
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
    use crate::scheduler::core::commit::aggregate::ProcessSnapshot;
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
    use crate::scheduler::core::recovery::recovery_index::{RecoveryIndex, RecoveryIndexCodec};
    use crate::scheduler::durable::document::ProcessDocumentCodec;
    use std::collections::{BTreeMap, HashMap};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// One version of an aggregate whose derived recovery index is present only
    /// when it carries the non-terminal operation: two such snapshots are two
    /// versions of the same aggregate as far as a load's consistency check is
    /// concerned.
    fn snapshot(with_operation: bool) -> ProcessSnapshot {
        let task = TaskId::new("step", 0).unwrap();
        let operation_id = OperationId {
            process_id: "process".into(),
            task: task.clone(),
            sequence: 0,
        };
        let claim = ClaimToken {
            owner: "runtime-a".into(),
            fence: 1,
            lease_until_ms: 9_000,
            heartbeat_revision: 0,
        };
        ProcessSnapshot {
            process_id: "process".into(),
            version: ProcessVersion {
                graph: 1,
                process: 2,
                operations: u64::from(with_operation),
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
        process: Vec<u8>,
        torn_index: Vec<u8>,
        torn: usize,
        reads: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl AsyncDocumentStore for TornIndexStore {
        async fn read_many(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>, ActError> {
            let read = self.reads.fetch_add(1, Ordering::SeqCst);
            let index = (read < self.torn).then(|| self.torn_index.clone());
            assert_eq!(
                keys.len(),
                4,
                "aggregate + recovery index + outbox index + ready index"
            );
            Ok(vec![Some(self.process.clone()), index, None, None])
        }

        async fn scan_prefix(&self, _prefix: &str) -> Result<Vec<(String, Vec<u8>)>, ActError> {
            unimplemented!("the load path does not scan")
        }

        async fn scan_range(
            &self,
            _prefix: &str,
            _lower: Option<&str>,
            _upper: Option<&str>,
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
            process: ProcessDocumentCodec::encode(&snapshot(false)).unwrap(),
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
        let repository = AsyncProcessRepository::new(store);
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
        let repository = AsyncProcessRepository::new(store);
        let error = repository.load("process").await.unwrap_err();
        assert!(
            format!("{error}").contains("RecoveryIndexMismatch"),
            "damage is still surfaced: {error}"
        );
        assert_eq!(reads.load(Ordering::SeqCst), LOAD_ATTEMPTS);
    }

    /// An operation a writer from *before* the operation's close was folded into its
    /// effect commit left `Committed` — the effect durable, the completion
    /// lost, which used to be a commit of its own — must still be finalized by
    /// recovery: the policy routes that state to `FinalizeCommit`, and the
    /// `Complete` event it carries applies to the durable record
    /// (`Committed` → `Completed`), so a store an older engine left behind
    /// converges instead of being stranded. No operation produces the state any more,
    /// but the V2 documents keep it.
    #[tokio::test]
    async fn a_committed_operation_left_by_an_older_writer_is_finalized() {
        use crate::scheduler::core::model::operation::{CommitToken, OperationEvent};
        use crate::scheduler::core::recovery::planner::{RecoveryDisposition, RecoveryPlanner};

        let claim = ClaimToken {
            owner: "runtime-a".into(),
            fence: 1,
            lease_until_ms: 9_000,
            heartbeat_revision: 0,
        };
        let commit = CommitToken {
            process_version: 3,
            digest: "legacy".into(),
        };
        let operation_id = OperationId {
            process_id: "process".into(),
            task: TaskId::new("step", 0).unwrap(),
            sequence: 0,
        };
        // The pair the pre-fold operation committed in two rounds: claim-and-prepare,
        // then the effect commit.
        let committed = OperationRecord::pending(operation_id.clone(), OperationKind::ExecuteTask)
            .evolve(OperationEvent::ClaimAndPrepare(claim.clone()))
            .unwrap()
            .evolve(OperationEvent::Commit {
                claim,
                commit: commit.clone(),
            })
            .unwrap();
        assert!(matches!(committed.state, OperationState::Committed(_)));

        let mut process = snapshot(false);
        process.version.operations = committed.revision;
        process
            .operations
            .insert(operation_id.clone(), committed.clone());

        let store: Arc<dyn KvStore> = Arc::new(crate::store::MemoryStore::new());
        let repository = AsyncProcessRepository::new(Arc::new(KvDocumentStore::new(store)));
        repository.create(&process, 0, 800).await.unwrap();

        assert_eq!(
            RecoveryPlanner::classify(&committed, 0),
            RecoveryDisposition::FinalizeCommit,
            "the older writer's state is finalized, not re-driven"
        );
        let outcome = repository
            .commit_operation_event(
                &operation_id,
                committed.revision,
                OperationEvent::Complete {
                    commit: commit.clone(),
                },
                0,
                800,
            )
            .await
            .unwrap();
        let OperationCommitOutcome::Committed { operation, .. } = outcome else {
            panic!("the finalize must commit, not lose a compare-and-swap: {outcome:?}");
        };
        assert_eq!(operation.state, OperationState::Completed(commit));
    }

    fn ready_snapshot(process_id: &str) -> (ProcessSnapshot, OperationId) {
        let task = TaskId::new("step", 0).unwrap();
        let operation = OperationId {
            process_id: process_id.into(),
            task: task.clone(),
            sequence: 0,
        };
        let mut process = snapshot(false);
        process.process_id = process_id.into();
        process.tasks.insert(
            task,
            TaskSnapshot {
                model_node: NodeId::parse("model-step").unwrap(),
                parent: None,
                predecessors: vec![],
                state: TaskState::Ready,
                revision: 0,
            },
        );
        process.operations.insert(
            operation.clone(),
            OperationRecord::pending(operation.clone(), OperationKind::ExecuteTask),
        );
        (process, operation)
    }

    fn repository_with_store() -> (AsyncProcessRepository, Arc<dyn KvStore>) {
        let store: Arc<dyn KvStore> = Arc::new(crate::store::MemoryStore::new());
        (
            AsyncProcessRepository::new(Arc::new(KvDocumentStore::new(store.clone()))),
            store,
        )
    }

    /// The ready record is installed by the create transaction and removed by
    /// the claim transaction: a crash between aggregate and index cannot happen
    /// because both are one guarded batch.
    #[tokio::test]
    async fn the_ready_record_moves_with_the_operation_state() {
        let (process, id) = ready_snapshot("process");
        let (repository, _) = repository_with_store();
        repository.create(&process, 100, 800).await.unwrap();

        let page = repository.due_ready_page(None, 100, 8).await.unwrap();
        assert_eq!(page.operations, vec![id.clone()]);
        assert_eq!(page.observed, 1);

        let claim = ClaimToken {
            owner: "runtime-a".into(),
            fence: 1,
            lease_until_ms: 9_000,
            heartbeat_revision: 0,
        };
        repository
            .commit_operation_event(
                &id,
                0,
                crate::scheduler::core::model::operation::OperationEvent::ClaimAndPrepare(
                    claim.clone(),
                ),
                100,
                800,
            )
            .await
            .unwrap();
        assert!(
            repository
                .due_ready_page(None, 100, 8)
                .await
                .unwrap()
                .operations
                .is_empty(),
            "the claim removes the record in the same batch"
        );

        repository
            .commit_operation_event(
                &id,
                1,
                crate::scheduler::core::model::operation::OperationEvent::ScheduleRetry {
                    claim,
                    attempt: 1,
                },
                200,
                800,
            )
            .await
            .unwrap();
        let page = repository.due_ready_page(None, 200, 8).await.unwrap();
        assert_eq!(page.operations, vec![id]);
        assert_eq!(
            page.oldest_ready_at_ms,
            Some(200),
            "the retry is due at its commit"
        );
    }

    /// A timer is in the ready index but not due until its first poll; recovery
    /// never sees it as exceptional work.
    #[tokio::test]
    async fn a_timer_waits_for_its_poll_and_is_not_recovery_work() {
        let (mut process, id) = ready_snapshot("process");
        process.operations.get_mut(&id).unwrap().kind = OperationKind::DispatchTimeout;
        assert!(
            crate::scheduler::core::recovery::recovery_index::RecoveryIndex::project(&process)
                .is_none(),
            "an idle timer is not recovery work"
        );
        let (repository, _) = repository_with_store();
        repository.create(&process, 100, 800).await.unwrap();
        assert!(
            repository
                .due_ready_page(None, 100, 8)
                .await
                .unwrap()
                .operations
                .is_empty()
        );
        assert_eq!(
            repository
                .due_ready_page(None, 900, 8)
                .await
                .unwrap()
                .operations,
            vec![id]
        );
    }

    /// A missing ready document is rebuilt from the authoritative aggregate;
    /// the repair is idempotent.
    #[tokio::test]
    async fn a_missing_ready_record_is_repaired_from_the_aggregate() {
        let (process, id) = ready_snapshot("process");
        let (repository, store) = repository_with_store();
        repository.create(&process, 100, 800).await.unwrap();
        store
            .delete(&ready_index_key("process").unwrap())
            .await
            .unwrap();
        assert!(
            repository
                .due_ready_page(None, 100, 8)
                .await
                .unwrap()
                .operations
                .is_empty()
        );
        assert!(repository.repair_ready("process", 100, 800).await.unwrap());
        assert_eq!(
            repository
                .due_ready_page(None, 100, 8)
                .await
                .unwrap()
                .operations,
            vec![id]
        );
        assert!(
            !repository.repair_ready("process", 100, 800).await.unwrap(),
            "a consistent index is left untouched"
        );
    }

    /// Recovery pagination resumes at its cursor instead of rescanning the
    /// documents before it, and the pages are disjoint.
    #[tokio::test]
    async fn recovery_pages_resume_at_the_cursor() {
        let (repository, _) = repository_with_store();
        let repository = Arc::new(repository);
        let recovery = AsyncRecoveryRepository::new(repository.clone());
        for process_id in ["p-a", "p-b", "p-c"] {
            let (mut process, id) = ready_snapshot(process_id);
            let claim = ClaimToken {
                owner: "runtime-a".into(),
                fence: 1,
                lease_until_ms: 9_000,
                heartbeat_revision: 0,
            };
            process.operations.insert(
                id.clone(),
                OperationRecord::pending(id.clone(), OperationKind::ExecuteTask)
                    .evolve(
                        crate::scheduler::core::model::operation::OperationEvent::ClaimAndPrepare(
                            claim,
                        ),
                    )
                    .unwrap(),
            );
            repository.create(&process, 0, 800).await.unwrap();
        }
        let mut seen = Vec::new();
        let mut cursor = None;
        loop {
            let page = recovery.recoverable_page(cursor.as_ref(), 1).await.unwrap();
            for operation in page.operations {
                assert!(
                    !seen.contains(&operation.id),
                    "a pagination walk may not repeat an operation"
                );
                seen.push(operation.id);
            }
            match page.next {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        assert_eq!(seen.len(), 3);
    }
}
