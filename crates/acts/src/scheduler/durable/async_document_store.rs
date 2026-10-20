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
                outbox_repository::{
                    OutboxBatchOutcome, OutboxConfirmation, OutboxCursor, OutboxPage,
                    OutboxRepositoryError,
                },
                records::{OutboxEvent, OutboxId, OutboxRecord},
            },
            ready::{ReadyIndex, ReadyIndexCodec},
            recovery::recovery_index::RecoveryIndexCodec,
        },
        durable::{
            document_store::{
                DocumentGuard, DocumentMutation, DocumentRepositoryError, InstallWrite,
                LoadedProcess, OUTBOX_INDEX_KEY_PREFIX, PROCESS_KEY_PREFIX, READY_INDEX_KEY_PREFIX,
                RECOVERY_INDEX_KEY_PREFIX, ReplaceWrite, build_create_transaction,
                build_migration_transaction, build_orphan_reclaim_transaction,
                build_outbox_confirm_transaction, build_reclaim_transaction,
                build_repair_transaction, build_replace_transaction, decode_loaded_process,
                decode_process_key, install_migration, loaded_after_write, outbox_index_key,
                process_base_key, process_key, process_log_prefix, ready_index_key,
                recovery_entries, recovery_entry_process_prefix, recovery_index_document,
                recovery_index_key,
            },
            snapshot_codec::LogChanges,
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
    /// The last aggregate decoded per process, keyed by pid and validated by
    /// the exact head bytes before use (see [`Self::load`]).
    ///
    /// Loading an aggregate decodes its base snapshot and replays its log —
    /// work linear in the run's whole history, paid twice by every operation
    /// (the claim and its plan commit). The head document is the aggregate's
    /// version token: every commit writes a new one, so a cached entry is
    /// reused only while the head read back still equals the bytes it was
    /// decoded from. Anything that moves the aggregate — this process or
    /// another writer — misses the cache and takes the full decode path.
    cached: parking_lot::Mutex<HashMap<String, Arc<LoadedProcess>>>,
}

/// Loaded aggregates kept resident: enough for the runs a driver works through
/// in one pass, bounded so a long-lived high-concurrency server does not retain
/// every run's history.
const LOAD_CACHE_CAP: usize = 64;

impl AsyncProcessRepository {
    pub(crate) fn new(store: Arc<dyn AsyncDocumentStore>) -> Self {
        Self {
            store,
            cached: parking_lot::Mutex::new(HashMap::new()),
        }
    }

    /// Publish a just-committed aggregate to the load cache. Its document
    /// bytes are the ones the commit wrote, so the next `load` hits while the
    /// head still names them.
    fn cache_loaded(&self, loaded: &LoadedProcess) {
        let mut cache = self.cached.lock();
        if cache.len() >= LOAD_CACHE_CAP
            && !cache.contains_key(&loaded.snapshot.process_id)
            && let Some(evicted) = cache.keys().next().cloned()
        {
            cache.remove(&evicted);
        }
        cache.insert(loaded.snapshot.process_id.clone(), Arc::new(loaded.clone()));
    }

    fn forget_loaded(&self, process_id: &str) {
        self.cached.lock().remove(process_id);
    }

    pub(crate) async fn create(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        now_ms: i64,
        poll_ms: i64,
    ) -> Result<LoadedProcess, AsyncRepositoryError> {
        let InstallWrite { transaction, write } =
            build_create_transaction(snapshot, now_ms, poll_ms)?;
        if !self
            .store
            .atomic_batch(transaction.guards(), transaction.mutations())
            .await?
        {
            return Err(AsyncRepositoryError::Conflict(snapshot.process_id.clone()));
        }
        let loaded = loaded_after_write(snapshot, write);
        self.cache_loaded(&loaded);
        Ok(loaded)
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
            operations.push(
                crate::scheduler::core::commit::aggregate::RecoveryCandidate {
                    operation: operation.clone(),
                    version: process.snapshot.version.clone(),
                },
            );
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
        let loaded = self.load(&id.process_id).await?.ok_or_else(|| {
            AsyncRepositoryError::Domain(CommitError::ProcessNotFound(id.process_id.clone()))
        })?;
        self.commit_operation_event_from(loaded, id, expected_revision, event, now_ms, poll_ms)
            .await
    }

    /// [`Self::commit_operation_event`] for a caller that has just loaded the
    /// same aggregate under the pid's command lane (the claim path). The
    /// loaded documents are the CAS token, so the write is exactly as guarded
    /// as a fresh read; a stale observation loses its guard and is re-read /
    /// reported as a conflict the same way. This saves one whole aggregate
    /// load per claim.
    pub(crate) async fn commit_operation_event_from(
        &self,
        mut loaded: LoadedProcess,
        id: &crate::scheduler::core::model::operation::OperationId,
        expected_revision: u64,
        event: crate::scheduler::core::model::operation::OperationEvent,
        now_ms: i64,
        poll_ms: i64,
    ) -> Result<OperationCommitOutcome, AsyncRepositoryError> {
        if loaded.snapshot.process_id != id.process_id {
            return Err(AsyncRepositoryError::Domain(CommitError::ForeignOperation(
                id.clone(),
            )));
        }
        // The advance runs on the loaded aggregate itself: this repository owns
        // it and drops it on failure (the caller reports and recovery
        // re-derives the operation), so the whole-aggregate clone is not needed.
        let entries = recovery_entries(&loaded.snapshot);
        let operation = advance_operation_event(&mut loaded.snapshot, id, expected_revision, event)
            .map_err(AsyncRepositoryError::Domain)?;
        let ready = self.ready_document(&loaded, now_ms, poll_ms, &HashMap::new())?;
        let changes = LogChanges::operation(id.clone());
        let outcome = self
            .replace(
                &loaded,
                &loaded.snapshot,
                &entries,
                ready.as_deref(),
                &changes,
            )
            .await?;
        match outcome {
            ReplaceOutcome::Committed(write) => {
                loaded.install_replace_documents(write);
                self.cache_loaded(&loaded);
                Ok(OperationCommitOutcome::Committed { operation, loaded })
            }
            ReplaceOutcome::AlreadyCommitted(process) => {
                let operation = authoritative_operation(&process, id)?;
                self.cache_loaded(&process);
                Ok(OperationCommitOutcome::AlreadyCommitted {
                    operation,
                    loaded: *process,
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
        let loaded = self.load(&plan.process_id).await?.ok_or_else(|| {
            AsyncRepositoryError::Domain(CommitError::ProcessNotFound(plan.process_id.clone()))
        })?;
        self.commit_transition_from(loaded, plan, now_ms, poll_ms)
            .await
    }

    /// Apply a pure domain transition to an aggregate the caller already
    /// holds. The claim's commit hands its `LoadedProcess` back, so the
    /// operation's plan is committed with that copy instead of decoding the
    /// whole aggregate a second time; its documents are the CAS token, and the
    /// command lane guarantees the claim is the last writer of this pid. A
    /// concurrent writer surfaces as a version conflict here, exactly as it
    /// does after a fresh load.
    pub(crate) async fn commit_transition_from(
        &self,
        mut loaded: LoadedProcess,
        plan: &crate::scheduler::core::model::transition_plan::TransitionPlan,
        now_ms: i64,
        poll_ms: i64,
    ) -> Result<TransitionCommitOutcome, AsyncRepositoryError> {
        if loaded.snapshot.process_id != plan.process_id {
            return Err(AsyncRepositoryError::Domain(CommitError::ProcessNotFound(
                plan.process_id.clone(),
            )));
        }
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
        let changes = LogChanges::from_transition(plan);
        let outcome = self
            .replace(
                &loaded,
                &loaded.snapshot,
                &entries,
                ready.as_deref(),
                &changes,
            )
            .await?;
        match outcome {
            ReplaceOutcome::Committed(write) => {
                loaded.install_replace_documents(write);
                self.cache_loaded(&loaded);
                Ok(TransitionCommitOutcome::Committed { receipt, loaded })
            }
            ReplaceOutcome::AlreadyCommitted(process) => {
                self.cache_loaded(&process);
                Ok(TransitionCommitOutcome::AlreadyCommitted {
                    receipt,
                    loaded: *process,
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

    /// Every durable process id, in key order. The boot catch-up of the read
    /// model (A5) walks this list once per host; it is the durable repair
    /// queue's authoritative membership (the in-memory queue is only a
    /// convenience view rebuilt from it).
    pub(crate) async fn process_ids(&self) -> Result<Vec<String>, AsyncRepositoryError> {
        let mut ids = Vec::new();
        for (key, _) in self.store.scan_prefix(PROCESS_KEY_PREFIX).await? {
            // The base and log entries live under the head's prefix; only the
            // head itself names a process.
            if crate::scheduler::durable::document_store::is_process_head_key(&key) {
                ids.push(decode_process_key(&key)?);
            }
        }
        Ok(ids)
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
            if !crate::scheduler::durable::document_store::is_process_head_key(&key) {
                continue;
            }
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
        let mut observed = 0usize;
        let mut due = 0usize;
        let mut oldest_due_at_ms: Option<i64> = None;
        for (key, document) in documents {
            let index = OutboxIndexCodec::decode(&document).map_err(|error| {
                OutboxRepositoryError::CorruptData(format!("outbox index {key:?}: {error}"))
            })?;
            observed += index.entries.len();
            for entry in index.entries {
                let available_at = match entry.state {
                    OutboxIndexState::Pending { available_at_ms } => available_at_ms,
                    // A claim whose lease expired is work again: its owner never
                    // confirmed it, and the record would otherwise sit `Claimed`
                    // forever (see `OutboxState::is_dispatchable`).
                    OutboxIndexState::Claimed { lease_until_ms } => lease_until_ms,
                    OutboxIndexState::DeliveryUnknown => continue,
                };
                if now_ms >= available_at {
                    due += 1;
                    oldest_due_at_ms = Some(
                        oldest_due_at_ms.map_or(available_at, |oldest| oldest.min(available_at)),
                    );
                    entries.push(entry);
                }
            }
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
                    let Some(loaded) = self
                        .load(&process_id)
                        .await
                        .map_err(|error| OutboxRepositoryError::Unavailable(error.to_string()))?
                    else {
                        // An interrupted cleanup left an index document whose
                        // aggregate is already gone. There is no record to
                        // deliver: skip it instead of failing the pass (and
                        // every later one) on work that cannot exist. The
                        // cleanup retry removes the document when it can name
                        // the process from its rows.
                        tracing::debug!(
                            process_id,
                            "outbox index references a missing aggregate; skipping"
                        );
                        continue;
                    };
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
        Ok(OutboxPage {
            records,
            next,
            observed,
            due,
            oldest_due_at_ms,
        })
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
        let changes = LogChanges::outbox([id.clone()]);
        match self
            .replace(
                &loaded,
                &loaded.snapshot,
                &entries,
                ready.as_deref(),
                &changes,
            )
            .await
        {
            Ok(ReplaceOutcome::Committed(write)) => {
                loaded.install_replace_documents(write);
                self.cache_loaded(&loaded);
                Ok(evolved)
            }
            Ok(ReplaceOutcome::AlreadyCommitted(_)) => Ok(evolved),
            Ok(ReplaceOutcome::Conflict) => {
                Err(OutboxRepositoryError::ConcurrentModification(id.clone()))
            }
            Err(error) => Err(OutboxRepositoryError::Unavailable(error.to_string())),
        }
    }

    /// Persist one operation's post-delivery outcomes in **one** guarded
    /// aggregate commit: every record the transport settled advances from
    /// `Pending` (claim and outcome applied in memory) to its final state, and
    /// the aggregate plus its outbox index are replaced under the exact bytes
    /// the caller's plan commit just wrote. The recovery and ready indexes are
    /// guarded but not rewritten — a confirmation changes no operation.
    ///
    /// A lost guard applies nothing; the records stay `Pending` and the outbox
    /// pass redelivers them. Records whose revision moved since the caller read
    /// them are skipped: a concurrent pass owns them now.
    pub(crate) async fn confirm_outbox_batch(
        &self,
        loaded: &mut LoadedProcess,
        confirmations: &[OutboxConfirmation],
    ) -> Result<OutboxBatchOutcome, OutboxRepositoryError> {
        if confirmations.is_empty() {
            return Ok(OutboxBatchOutcome {
                confirmed: Vec::new(),
            });
        }
        let process_id = loaded.snapshot.process_id.clone();
        let mut confirmed = Vec::new();
        let mut previous = Vec::new();
        for confirmation in confirmations {
            let Some(record) = loaded.snapshot.outbox.get(&confirmation.id).cloned() else {
                continue;
            };
            if record.revision != confirmation.expected_revision {
                continue;
            }
            let mut evolved = record.clone();
            let mut applicable = true;
            for event in &confirmation.events {
                match evolved.evolve(event.clone()) {
                    Ok(next) => evolved = next,
                    Err(_) => {
                        applicable = false;
                        break;
                    }
                }
            }
            if !applicable {
                continue;
            }
            loaded
                .snapshot
                .outbox
                .insert(confirmation.id.clone(), evolved.clone());
            previous.push((confirmation.id.clone(), record));
            confirmed.push(evolved);
        }
        if confirmed.is_empty() {
            return Ok(OutboxBatchOutcome { confirmed });
        }
        let confirmed_ids = confirmed
            .iter()
            .map(|record| record.id.clone())
            .collect::<Vec<_>>();
        let write = match build_outbox_confirm_transaction(loaded, &loaded.snapshot, confirmed_ids)
        {
            Ok(write) => write,
            Err(error) => {
                restore_records(&mut loaded.snapshot, previous);
                return Err(OutboxRepositoryError::Unavailable(error.to_string()));
            }
        };
        let applied = match self
            .store
            .atomic_batch(write.transaction.guards(), write.transaction.mutations())
            .await
        {
            Ok(applied) => applied,
            Err(error) => {
                restore_records(&mut loaded.snapshot, previous);
                return Err(OutboxRepositoryError::Unavailable(error.to_string()));
            }
        };
        if applied {
            loaded.install_confirmed_documents(write.write);
            self.cache_loaded(loaded);
            return Ok(OutboxBatchOutcome { confirmed });
        }
        // The guard moved. If the authoritative aggregate already equals this
        // replacement, a concurrent copy of the confirmation won; otherwise
        // the caller leaves the records pending for the outbox pass.
        let current = self
            .load(&process_id)
            .await
            .map_err(|error| OutboxRepositoryError::Unavailable(error.to_string()))?;
        if current.is_some_and(|current| current.snapshot == loaded.snapshot) {
            loaded.install_confirmed_documents(write.write);
            self.cache_loaded(loaded);
            Ok(OutboxBatchOutcome { confirmed })
        } else {
            restore_records(&mut loaded.snapshot, previous);
            Err(OutboxRepositoryError::ConcurrentProcess(process_id))
        }
    }

    pub(crate) async fn replace(
        &self,
        loaded: &LoadedProcess,
        replacement_snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        current_recovery_entries: &std::collections::BTreeMap<String, Vec<u8>>,
        replacement_ready: Option<&[u8]>,
        changes: &LogChanges,
    ) -> Result<ReplaceOutcome, AsyncRepositoryError> {
        let InstallWrite { transaction, write } = build_replace_transaction(
            loaded,
            replacement_snapshot,
            current_recovery_entries,
            replacement_ready,
            changes,
        )?;
        if self
            .store
            .atomic_batch(transaction.guards(), transaction.mutations())
            .await?
        {
            return Ok(ReplaceOutcome::Committed(write));
        }
        let authoritative = self.load(&loaded.snapshot.process_id).await?;
        match authoritative {
            Some(current) if current.snapshot == *replacement_snapshot => {
                Ok(ReplaceOutcome::AlreadyCommitted(Box::new(current)))
            }
            _ => Ok(ReplaceOutcome::Conflict),
        }
    }

    /// Atomically reclaim one run whose aggregate is already
    /// [reclaimable](crate::scheduler::core::commit::aggregate::ProcessSnapshot::is_reclaimable):
    /// delete the aggregate and its recovery, outbox and ready index documents
    /// in one guarded batch (the aggregate's exact bytes are the version
    /// guard). Leftover per-operation recovery-entry documents are deleted in
    /// the same batch.
    ///
    /// When the aggregate is already gone — an interrupted earlier cleanup —
    /// the derived index documents still present are removed, guarded by the
    /// aggregate's absence. `Absent` with nothing to remove is a clean no-op,
    /// so the caller can distinguish "never existed / fully reclaimed" from
    /// "index or row residue remains".
    ///
    /// A concurrent commit, start or another cleaner makes the guarded batch
    /// fail; the load is then repeated. After [`LOAD_ATTEMPTS`] the outcome is
    /// a conflict, which the cleanup loop reports and retries on its next
    /// pass; the aggregate is never deleted on an unguarded read.
    pub(crate) async fn reclaim(
        &self,
        process_id: &str,
    ) -> Result<ReclaimOutcome, AsyncRepositoryError> {
        let entry_prefix = recovery_entry_process_prefix(process_id)?;
        for _ in 0..LOAD_ATTEMPTS {
            let loaded = self.load(process_id).await?;
            match loaded {
                Some(loaded) => {
                    if !loaded.snapshot.is_reclaimable() {
                        return Ok(ReclaimOutcome::Retained);
                    }
                    let recovery_entries = self.store.scan_prefix(&entry_prefix).await?;
                    let transaction = build_reclaim_transaction(&loaded, &recovery_entries)?;
                    let document_bytes = loaded.original_document().len()
                        + loaded.original_base().map_or(0, <[u8]>::len)
                        + loaded.original_log().values().map(Vec::len).sum::<usize>()
                        + loaded.original_recovery_index().map_or(0, <[u8]>::len)
                        + loaded.original_outbox_index().map_or(0, <[u8]>::len)
                        + loaded.original_ready_index().map_or(0, <[u8]>::len)
                        + recovery_entries
                            .iter()
                            .map(|(_, bytes)| bytes.len())
                            .sum::<usize>();
                    if self
                        .store
                        .atomic_batch(transaction.guards(), transaction.mutations())
                        .await?
                    {
                        self.forget_loaded(process_id);
                        return Ok(ReclaimOutcome::Reclaimed {
                            document_bytes: document_bytes as u64,
                            orphan_indexes: recovery_entries.len(),
                        });
                    }
                }
                None => {
                    let recovery_entries = self.store.scan_prefix(&entry_prefix).await?;
                    let log_entries = self
                        .store
                        .scan_prefix(&process_log_prefix(process_id)?)
                        .await?;
                    let keys = [
                        process_base_key(process_id)?,
                        recovery_index_key(process_id)?,
                        outbox_index_key(process_id)?,
                        ready_index_key(process_id)?,
                    ];
                    let mut documents = self.store.read_many(&keys).await?.into_iter();
                    let base = documents.next().flatten();
                    let recovery_index = documents.next().flatten();
                    let outbox_index = documents.next().flatten();
                    let ready_index = documents.next().flatten();
                    let orphan_indexes = [&base, &recovery_index, &outbox_index, &ready_index]
                        .into_iter()
                        .filter(|document| document.is_some())
                        .count()
                        + log_entries.len()
                        + recovery_entries.len();
                    if orphan_indexes == 0 {
                        self.forget_loaded(process_id);
                        return Ok(ReclaimOutcome::Absent {
                            document_bytes: 0,
                            orphan_indexes: 0,
                        });
                    }
                    let document_bytes = base.as_ref().map_or(0, Vec::len)
                        + recovery_index.as_ref().map_or(0, Vec::len)
                        + outbox_index.as_ref().map_or(0, Vec::len)
                        + ready_index.as_ref().map_or(0, Vec::len)
                        + log_entries
                            .iter()
                            .map(|(_, bytes)| bytes.len())
                            .sum::<usize>()
                        + recovery_entries
                            .iter()
                            .map(|(_, bytes)| bytes.len())
                            .sum::<usize>();
                    let transaction = build_orphan_reclaim_transaction(
                        process_id,
                        base,
                        &log_entries,
                        recovery_index,
                        outbox_index,
                        ready_index,
                        &recovery_entries,
                    )?;
                    if self
                        .store
                        .atomic_batch(transaction.guards(), transaction.mutations())
                        .await?
                    {
                        self.forget_loaded(process_id);
                        return Ok(ReclaimOutcome::Absent {
                            document_bytes: document_bytes as u64,
                            orphan_indexes,
                        });
                    }
                }
            }
        }
        Err(AsyncRepositoryError::Conflict(process_id.to_string()))
    }

    /// Upgrade a legacy whole-snapshot aggregate to the L8 family. A store
    /// write failure is not fatal to reads: the caller keeps serving the
    /// legacy snapshot and the next load retries. A guard loss means another
    /// load migrated first, so the caller re-reads.
    async fn migrate_legacy(
        &self,
        loaded: LoadedProcess,
    ) -> Result<MigrationOutcome, AsyncRepositoryError> {
        let (transaction, head, base) = build_migration_transaction(&loaded)?;
        match self
            .store
            .atomic_batch(transaction.guards(), transaction.mutations())
            .await
        {
            Ok(true) => Ok(MigrationOutcome::Migrated(install_migration(
                loaded, head, base,
            ))),
            Ok(false) => Ok(MigrationOutcome::Conflict),
            Err(error) => {
                tracing::warn!(
                    process_id = loaded.snapshot.process_id,
                    error = %error,
                    "legacy aggregate migration failed; keeping the legacy document readable"
                );
                Ok(MigrationOutcome::Failed(loaded))
            }
        }
    }

    /// Load one aggregate family with its derived indexes.
    ///
    /// The head, base and indexes are read in one `read_many` call and the log
    /// entries in one prefix scan. A store without a native batch read serves
    /// the batch key by key, so a commit landing between reads can pair a head
    /// with records from another version. The head is re-read after the log
    /// scan and compared byte for byte — any commit installs a different head
    /// (its epoch advances) — and the derived indexes are checked against the
    /// rebuilt snapshot; either disagreement retries the read. A mismatch that
    /// survives the attempts is surfaced as damage.
    pub(crate) async fn load(
        &self,
        process_id: &str,
    ) -> Result<Option<LoadedProcess>, AsyncRepositoryError> {
        let head_key = process_key(process_id)?;
        let cached = self.cached.lock().get(process_id).cloned();
        if let Some(cached) = cached {
            // The head is the version token and the three derived documents
            // are guards of it: a hit means this process's aggregate is
            // exactly the state that was decoded, down to the documents a
            // repair pass may have rewritten. Reading those five documents
            // replaces decoding the base snapshot and every log entry.
            let keys = [
                head_key.clone(),
                recovery_index_key(process_id)?,
                outbox_index_key(process_id)?,
                ready_index_key(process_id)?,
                process_base_key(process_id)?,
            ];
            let mut documents = self.store.read_many(&keys).await?.into_iter();
            let head = documents.next().flatten();
            let recovery_index = documents.next().flatten();
            let outbox_index = documents.next().flatten();
            let ready_index = documents.next().flatten();
            let base = documents.next().flatten();
            if head.as_deref() == Some(cached.original_document())
                && recovery_index.as_deref() == cached.original_recovery_index()
                && outbox_index.as_deref() == cached.original_outbox_index()
                && ready_index.as_deref() == cached.original_ready_index()
                && base.as_deref() == cached.original_base()
            {
                // The head, base and every derived document are the exact
                // bytes this aggregate was decoded from. Log entries are
                // immutable for a head (a commit that appends one writes a
                // new epoch), so the cached replay is still the authoritative
                // snapshot and the decode/verify pass can be skipped.
                return Ok(Some((*cached).clone()));
            }
        }
        let keys = vec![
            head_key,
            recovery_index_key(process_id)?,
            outbox_index_key(process_id)?,
            ready_index_key(process_id)?,
            process_base_key(process_id)?,
        ];
        let log_prefix = process_log_prefix(process_id)?;
        for attempt in 0..LOAD_ATTEMPTS {
            let mut documents = self.store.read_many(&keys).await?.into_iter();
            let process = documents.next().flatten();
            let recovery_index = documents.next().flatten();
            let outbox_index = documents.next().flatten();
            let ready_index = documents.next().flatten();
            let base = documents.next().flatten();
            let Some(process) = process else {
                self.forget_loaded(process_id);
                return Ok(None);
            };
            let log_entries = self.store.scan_prefix(&log_prefix).await?;
            let head_bytes = process.clone();
            match decode_loaded_process(
                process_id,
                process,
                base,
                &log_entries,
                recovery_index,
                outbox_index,
                ready_index,
            ) {
                Ok(loaded) if loaded.is_legacy() => match self.migrate_legacy(loaded).await? {
                    MigrationOutcome::Migrated(loaded) => {
                        self.cache_loaded(&loaded);
                        return Ok(Some(loaded));
                    }
                    MigrationOutcome::Failed(loaded) => {
                        // The legacy document stays readable, but the next
                        // load must retry the migration: caching it would
                        // pin the un-migrated read forever.
                        self.forget_loaded(process_id);
                        return Ok(Some(loaded));
                    }
                    MigrationOutcome::Conflict if attempt + 1 < LOAD_ATTEMPTS => {
                        tracing::debug!(
                            process_id,
                            attempt,
                            "legacy migration lost a guard; re-reading"
                        );
                    }
                    MigrationOutcome::Conflict => {
                        return Err(AsyncRepositoryError::Conflict(process_id.into()));
                    }
                },
                Ok(loaded) => {
                    // Prefer the newest consistent view: if a commit landed
                    // between the batch read and the log scan, retry. On the
                    // last attempt the decoded snapshot is still internally
                    // consistent (base/head/log all agree), so hand it back
                    // rather than failing a reader under a busy writer.
                    let current = self
                        .store
                        .read_many(&[process_key(process_id)?])
                        .await?
                        .into_iter()
                        .next()
                        .flatten();
                    if current.as_deref() == Some(head_bytes.as_slice()) {
                        self.cache_loaded(&loaded);
                        return Ok(Some(loaded));
                    }
                    if attempt + 1 < LOAD_ATTEMPTS {
                        tracing::debug!(
                            process_id,
                            attempt,
                            "aggregate head moved while reading its log; re-reading"
                        );
                        continue;
                    }
                    self.cache_loaded(&loaded);
                    return Ok(Some(loaded));
                }
                Err(error) => {
                    // A derived index can be read from another version; that
                    // is a plain race and retries without another head read. A
                    // compaction can also delete the log window between the
                    // batch read and the prefix scan, leaving an intact old
                    // head with no entries — so a non-race error is retried
                    // only when the head moved.
                    if error.is_read_race() {
                        if attempt + 1 < LOAD_ATTEMPTS {
                            tracing::debug!(
                                process_id,
                                attempt,
                                "aggregate read raced a writer; re-reading"
                            );
                            continue;
                        }
                        return Err(error.into());
                    }
                    let current = self
                        .store
                        .read_many(&[process_key(process_id)?])
                        .await?
                        .into_iter()
                        .next()
                        .flatten();
                    if current.as_deref() != Some(head_bytes.as_slice())
                        && attempt + 1 < LOAD_ATTEMPTS
                    {
                        tracing::debug!(
                            process_id,
                            attempt,
                            "aggregate head moved while reading its log; re-reading"
                        );
                        continue;
                    }
                    return Err(error.into());
                }
            }
        }
        unreachable!("the loop returns within LOAD_ATTEMPTS")
    }
}

enum MigrationOutcome {
    /// The legacy document was rewritten as an L8 head + base.
    Migrated(LoadedProcess),
    /// Another writer changed the head first; the caller re-reads.
    Conflict,
    /// The store refused the migration; the legacy snapshot stays readable.
    Failed(LoadedProcess),
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

/// Undo an in-memory confirmation whose guarded batch did not apply. The
/// caller's snapshot must keep describing storage, not a write that lost.
fn restore_records(
    snapshot: &mut crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    previous: Vec<(OutboxId, OutboxRecord)>,
) {
    for (id, record) in previous {
        snapshot.outbox.insert(id, record);
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
        /// The aggregate this commit installed, with its exact bytes. Handed
        /// back so a caller forms its plan from the committed state and — for
        /// recovery's finalize (A7) — runs the same post-commit program as the
        /// normal dispatch, without a second load that could already be older.
        loaded: LoadedProcess,
    },
    AlreadyCommitted {
        operation: crate::scheduler::core::model::operation::OperationRecord,
        /// The authoritative aggregate another writer installed.
        loaded: LoadedProcess,
    },
    Conflict {},
}

#[derive(Debug)]
pub(crate) enum TransitionCommitOutcome {
    Committed {
        receipt: CommitReceipt,
        /// The exact written aggregate and index bytes; the post-delivery
        /// confirmation uses them as its guard instead of re-reading.
        loaded: LoadedProcess,
    },
    AlreadyCommitted {
        receipt: CommitReceipt,
        loaded: LoadedProcess,
    },
    Conflict {},
}

#[derive(Debug)]
pub(crate) enum ReplaceOutcome {
    /// The batch applied; the payload is the exact written aggregate plus the
    /// index bytes, so the caller can adopt them as its next CAS token without
    /// re-reading or re-encoding.
    Committed(ReplaceWrite),
    AlreadyCommitted(Box<LoadedProcess>),
    Conflict,
}

/// What one reclamation attempt did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReclaimOutcome {
    /// The aggregate and its indexes were deleted in one guarded batch.
    /// `orphan_indexes` counts leftover recovery-entry documents removed with
    /// them (a reclaimable aggregate projects none).
    Reclaimed {
        document_bytes: u64,
        orphan_indexes: usize,
    },
    /// The aggregate was already absent. `orphan_indexes` is non-zero when
    /// index documents of the gone aggregate were removed; zero means there
    /// was nothing left to clean.
    Absent {
        document_bytes: u64,
        orphan_indexes: usize,
    },
    /// The aggregate still exists and the run is not reclaimable (unsettled
    /// outbox, an outstanding operation, `EffectUnknown`/`Quarantined`, or
    /// tasks that are not terminal). Nothing was deleted.
    Retained,
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
    use crate::scheduler::durable::document_store::{MAX_LOG_ENTRIES, outbox_index_document};
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
        head: Vec<u8>,
        base: Vec<u8>,
        torn_index: Vec<u8>,
        torn: usize,
        reads: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl AsyncDocumentStore for TornIndexStore {
        async fn read_many(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>, ActError> {
            let read = self.reads.fetch_add(1, Ordering::SeqCst);
            if keys.len() == 1 {
                // The post-decode head re-read.
                return Ok(vec![Some(self.head.clone())]);
            }
            let index = (read < self.torn).then(|| self.torn_index.clone());
            assert_eq!(
                keys.len(),
                5,
                "head + recovery index + outbox index + ready index + base"
            );
            Ok(vec![
                Some(self.head.clone()),
                index,
                None,
                None,
                Some(self.base.clone()),
            ])
        }

        async fn scan_prefix(&self, _prefix: &str) -> Result<Vec<(String, Vec<u8>)>, ActError> {
            Ok(Vec::new())
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
        let source = snapshot(false);
        let head = crate::scheduler::durable::document::ProcessLogCodec::encode_head(
            &crate::scheduler::durable::snapshot_codec::HeadPayload {
                process_id: source.process_id.clone(),
                version: source.version.clone(),
                epoch: 0,
                base_seq: 0,
                next_seq: 0,
            },
        )
        .unwrap();
        let base =
            crate::scheduler::durable::document::ProcessLogCodec::encode_base(&source, 0).unwrap();
        let store = Arc::new(TornIndexStore {
            head,
            base,
            torn_index: RecoveryIndexCodec::encode(
                &RecoveryIndex::project(&snapshot(true)).unwrap(),
            )
            .unwrap(),
            torn,
            reads: reads.clone(),
        });
        (store, reads)
    }

    /// A run that is reclaimable by A11: its only execution is terminal, its
    /// outbox record is delivered and it has no outstanding operations.
    fn reclaimable_snapshot(process_id: &str) -> ProcessSnapshot {
        let task = TaskId::new("step", 0).unwrap();
        let outbox_id = crate::scheduler::core::outbox::records::OutboxId {
            process_id: process_id.into(),
            key: "delivered".into(),
        };
        ProcessSnapshot {
            process_id: process_id.into(),
            version: ProcessVersion {
                graph: 1,
                process: 1,
                operations: 0,
            },
            tasks: HashMap::from([(
                task,
                TaskSnapshot {
                    model_node: NodeId::parse("model-step").unwrap(),
                    parent: None,
                    predecessors: vec![],
                    state: TaskState::Completed,
                    revision: 1,
                },
            )]),
            variables: BTreeMap::new(),
            operations: HashMap::new(),
            outbox: HashMap::from([(
                outbox_id.clone(),
                crate::scheduler::core::outbox::records::OutboxRecord {
                    id: outbox_id,
                    topic: "topic".into(),
                    payload: "{}".into(),
                    state: crate::scheduler::core::outbox::records::OutboxState::Delivered {
                        receipt: "ok".into(),
                    },
                    attempt: 1,
                    last_fence: 1,
                    revision: 1,
                },
            )]),
        }
    }

    /// A terminal run with one non-retired operation: `EffectUnknown` and
    /// `Quarantined` must keep the aggregate and every index.
    fn retained_snapshot(process_id: &str, state: OperationState) -> ProcessSnapshot {
        let mut snapshot = reclaimable_snapshot(process_id);
        let operation_id = OperationId {
            process_id: process_id.into(),
            task: TaskId::new("step", 0).unwrap(),
            sequence: 0,
        };
        snapshot.version.operations = 1;
        snapshot.operations.insert(
            operation_id.clone(),
            OperationRecord {
                id: operation_id,
                kind: OperationKind::ExecuteTask,
                state,
                revision: 2,
            },
        );
        snapshot
    }

    /// The authoritative batch is guarded by the exact aggregate bytes the
    /// decision was taken on (its version), and a missing index is guarded
    /// absent instead of being deleted blind.
    #[test]
    fn reclaim_transaction_guards_the_aggregate_and_every_index() {
        let snapshot = reclaimable_snapshot("guard");
        let InstallWrite { write, .. } = build_create_transaction(&snapshot, 0, 800).unwrap();
        let loaded = loaded_after_write(&snapshot, write);
        let transaction = build_reclaim_transaction(&loaded, &[]).unwrap();
        let aggregate_key = process_key("guard").unwrap();
        let aggregate_guard = transaction
            .guards()
            .iter()
            .find(|guard| guard.key == aggregate_key)
            .expect("the aggregate must be guarded");
        assert_eq!(
            aggregate_guard.expected.as_deref(),
            Some(loaded.original_document())
        );
        let base_key = process_base_key("guard").unwrap();
        assert!(
            transaction.guards().iter().any(|guard| {
                guard.key == base_key && guard.expected.as_deref() == loaded.original_base()
            }),
            "the base snapshot must be guarded by its exact bytes"
        );
        for key in [
            recovery_index_key("guard").unwrap(),
            outbox_index_key("guard").unwrap(),
            ready_index_key("guard").unwrap(),
        ] {
            assert!(
                transaction
                    .guards()
                    .iter()
                    .any(|guard| guard.key == key && guard.expected.is_none()),
                "a missing index must be guarded absent: {key:?}"
            );
        }
        for key in [
            aggregate_key,
            process_base_key("guard").unwrap(),
            recovery_index_key("guard").unwrap(),
            outbox_index_key("guard").unwrap(),
            ready_index_key("guard").unwrap(),
        ] {
            assert!(
                transaction
                    .mutations()
                    .contains(&DocumentMutation::Delete { key: key.clone() }),
                "the batch must delete {key:?}"
            );
        }
    }

    #[tokio::test]
    async fn reclaim_deletes_the_guarded_documents_and_is_idempotent() {
        let kv = Arc::new(crate::store::MemoryStore::new());
        let repository = AsyncProcessRepository::new(Arc::new(KvDocumentStore::new(kv.clone())));
        let snapshot = reclaimable_snapshot("reclaim");
        repository
            .create(&snapshot, 0, 800)
            .await
            .expect("the aggregate is created");

        // A settled run projects no recovery/outbox/ready document, so the
        // aggregate family (head + base) is the only document present — and a
        // missing ready index (an older database) is still a clean delete.
        assert!(
            kv.one(&process_key("reclaim").unwrap())
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            kv.one(&process_base_key("reclaim").unwrap())
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            kv.one(&ready_index_key("reclaim").unwrap())
                .await
                .unwrap()
                .is_none()
        );
        let outcome = repository.reclaim("reclaim").await.unwrap();
        let debug = format!("{outcome:?}");
        assert!(debug.starts_with("Reclaimed"));
        assert!(debug.contains("document_bytes: "));
        assert!(!debug.contains("document_bytes: 0"), "{debug}");
        for key in [
            process_key("reclaim").unwrap(),
            process_base_key("reclaim").unwrap(),
            recovery_index_key("reclaim").unwrap(),
            outbox_index_key("reclaim").unwrap(),
            ready_index_key("reclaim").unwrap(),
        ] {
            assert!(
                kv.one(&key).await.unwrap().is_none(),
                "{key:?} must be gone"
            );
        }

        // A second pass finds nothing to do: reclamation is idempotent.
        assert_eq!(
            repository.reclaim("reclaim").await.unwrap(),
            ReclaimOutcome::Absent {
                document_bytes: 0,
                orphan_indexes: 0,
            }
        );
    }

    #[tokio::test]
    async fn reclaim_retains_effect_unknown_and_quarantined_runs() {
        let kv = Arc::new(crate::store::MemoryStore::new());
        let repository = AsyncProcessRepository::new(Arc::new(KvDocumentStore::new(kv.clone())));
        for (pid, state) in [
            (
                "effect-unknown",
                OperationState::EffectUnknown { attempt: 1 },
            ),
            (
                "quarantined",
                OperationState::Quarantined {
                    reason: "manual review".into(),
                },
            ),
        ] {
            let snapshot = retained_snapshot(pid, state);
            repository.create(&snapshot, 0, 800).await.unwrap();
            assert_eq!(
                repository.reclaim(pid).await.unwrap(),
                ReclaimOutcome::Retained,
                "{pid} must stay for reconciliation"
            );
            assert!(
                kv.one(&process_key(pid).unwrap()).await.unwrap().is_some(),
                "{pid}'s aggregate must survive"
            );
            assert!(
                kv.one(&recovery_index_key(pid).unwrap())
                    .await
                    .unwrap()
                    .is_some(),
                "{pid}'s recovery index must survive"
            );
        }
    }

    #[tokio::test]
    async fn reclaim_retains_outbox_records_that_did_not_deliver() {
        let kv = Arc::new(crate::store::MemoryStore::new());
        let repository = AsyncProcessRepository::new(Arc::new(KvDocumentStore::new(kv.clone())));
        for (pid, state) in [
            (
                "delivery-unknown",
                crate::scheduler::core::outbox::records::OutboxState::DeliveryUnknown {
                    reason: "transport outcome unknown".into(),
                },
            ),
            (
                "outbox-quarantined",
                crate::scheduler::core::outbox::records::OutboxState::Quarantined {
                    reason: "manual review".into(),
                },
            ),
            (
                "outbox-pending",
                crate::scheduler::core::outbox::records::OutboxState::Pending {
                    available_at_ms: 0,
                },
            ),
        ] {
            let mut snapshot = reclaimable_snapshot(pid);
            let record = snapshot
                .outbox
                .values_mut()
                .next()
                .expect("the snapshot carries one record");
            record.state = state;
            repository.create(&snapshot, 0, 800).await.unwrap();
            assert_eq!(
                repository.reclaim(pid).await.unwrap(),
                ReclaimOutcome::Retained,
                "{pid} must stay until its outbox record is delivered"
            );
            assert!(
                kv.one(&process_key(pid).unwrap()).await.unwrap().is_some(),
                "{pid}'s aggregate must survive"
            );
        }
    }

    /// A crash after the aggregate batch leaves the derived indexes behind.
    /// The retry recognises the missing aggregate and deletes exactly the
    /// documents that exist, including per-operation recovery entries — and
    /// nothing that belongs to another pid.
    #[tokio::test]
    async fn reclaim_cleans_orphan_indexes_after_the_aggregate_is_gone() {
        let kv = Arc::new(crate::store::MemoryStore::new());
        let repository = AsyncProcessRepository::new(Arc::new(KvDocumentStore::new(kv.clone())));
        let pid = "orphan";
        for key in [
            recovery_index_key(pid).unwrap(),
            outbox_index_key(pid).unwrap(),
            ready_index_key(pid).unwrap(),
        ] {
            kv.put(&key, b"stale index".to_vec()).await.unwrap();
        }
        let entry_key = format!("{}abc", recovery_entry_process_prefix(pid).unwrap());
        kv.put(&entry_key, b"stale entry".to_vec()).await.unwrap();
        let other_key = recovery_index_key("other").unwrap();
        kv.put(&other_key, b"not mine".to_vec()).await.unwrap();

        assert_eq!(
            repository.reclaim(pid).await.unwrap(),
            ReclaimOutcome::Absent {
                document_bytes: 3 * b"stale index".len() as u64 + b"stale entry".len() as u64,
                orphan_indexes: 4,
            }
        );
        for key in [
            recovery_index_key(pid).unwrap(),
            outbox_index_key(pid).unwrap(),
            ready_index_key(pid).unwrap(),
            entry_key,
        ] {
            assert!(
                kv.one(&key).await.unwrap().is_none(),
                "{key:?} must be gone"
            );
        }
        assert!(
            kv.one(&other_key).await.unwrap().is_some(),
            "another pid's index must not be touched"
        );
    }

    /// A store that commits a new aggregate version just before the reclaim
    /// batch applies: the exact-bytes guard must refuse the deletion, the
    /// retry must re-read and find the run no longer reclaimable, and neither
    /// the aggregate nor its indexes may be removed.
    struct RaceOnReclaimStore {
        inner: Arc<KvDocumentStore>,
        replacement: Vec<u8>,
        raced: std::sync::atomic::AtomicBool,
    }

    #[async_trait::async_trait]
    impl AsyncDocumentStore for RaceOnReclaimStore {
        async fn read_many(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>, ActError> {
            self.inner.read_many(keys).await
        }
        async fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, ActError> {
            self.inner.scan_prefix(prefix).await
        }
        async fn scan_range(
            &self,
            prefix: &str,
            lower: Option<&str>,
            upper: Option<&str>,
        ) -> Result<Vec<(String, Vec<u8>)>, ActError> {
            self.inner.scan_range(prefix, lower, upper).await
        }
        async fn atomic_batch(
            &self,
            guards: &[DocumentGuard],
            mutations: &[DocumentMutation],
        ) -> Result<bool, ActError> {
            let deletes_aggregate = mutations.iter().any(|mutation| match mutation {
                DocumentMutation::Delete { key } => key.starts_with(PROCESS_KEY_PREFIX),
                DocumentMutation::Put { .. } => false,
            });
            let raced = self.raced.swap(true, Ordering::SeqCst);
            if deletes_aggregate & !raced {
                // The commit the reclamation read did not see: it installs a
                // newer, non-reclaimable aggregate version.
                self.inner
                    .atomic_batch(
                        &[],
                        &[DocumentMutation::Put {
                            key: process_key("race").unwrap(),
                            value: self.replacement.clone(),
                        }],
                    )
                    .await?;
            }
            self.inner.atomic_batch(guards, mutations).await
        }
    }

    #[tokio::test]
    async fn reclaim_refuses_to_delete_an_aggregate_that_moved() {
        let kv = Arc::new(crate::store::MemoryStore::new());
        let inner = Arc::new(KvDocumentStore::new(kv.clone()));
        let creator = AsyncProcessRepository::new(inner.clone());
        creator
            .create(&reclaimable_snapshot("race"), 0, 800)
            .await
            .unwrap();

        let mut replacement = reclaimable_snapshot("race");
        replacement.version.process += 1;
        for task in replacement.tasks.values_mut() {
            task.state = TaskState::Running;
        }
        let replacement_document = ProcessDocumentCodec::encode(&replacement).unwrap();
        let repository = AsyncProcessRepository::new(Arc::new(RaceOnReclaimStore {
            inner,
            replacement: replacement_document.clone(),
            raced: std::sync::atomic::AtomicBool::new(false),
        }));
        assert_eq!(
            repository.reclaim("race").await.unwrap(),
            ReclaimOutcome::Retained,
            "the moved aggregate is no longer reclaimable"
        );
        assert_eq!(
            repository.load("race").await.unwrap().unwrap().snapshot,
            replacement,
            "the concurrent commit's aggregate must survive the reclaim attempt"
        );
        assert_ne!(
            kv.one(&process_key("race").unwrap())
                .await
                .unwrap()
                .unwrap(),
            replacement_document,
            "the legacy document the rival writer installed was migrated, not deleted"
        );
    }

    /// An outbox index document whose aggregate is already gone (an
    /// interrupted cleanup, an older orphan) is skipped, not turned into a
    /// corrupt-data failure that would wedge every later delivery pass.
    #[tokio::test]
    async fn outbox_page_skips_an_index_whose_aggregate_is_gone() {
        use crate::scheduler::core::outbox::outbox_index::OutboxIndex;

        let kv = Arc::new(crate::store::MemoryStore::new());
        let repository = AsyncProcessRepository::new(Arc::new(KvDocumentStore::new(kv.clone())));
        let pid = "gone";
        let outbox_id = crate::scheduler::core::outbox::records::OutboxId {
            process_id: pid.into(),
            key: "k".into(),
        };
        let record = crate::scheduler::core::outbox::records::OutboxRecord::pending(
            outbox_id.clone(),
            "topic".into(),
            "{}".into(),
            0,
        )
        .unwrap();
        let index = OutboxIndex::project(pid, &HashMap::from([(outbox_id, record)]))
            .unwrap()
            .expect("a pending record projects an index");
        kv.put(
            &outbox_index_key(pid).unwrap(),
            OutboxIndexCodec::encode(&index).unwrap(),
        )
        .await
        .unwrap();
        let page = repository
            .dispatchable_outbox_page(0, None, 10)
            .await
            .expect("a missing aggregate is stale, not damage");
        assert!(page.records.is_empty());
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
        assert_eq!(
            reads.load(Ordering::SeqCst),
            3,
            "the torn read was retried, then the head re-read verified the window"
        );
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
        use crate::scheduler::core::recovery::planner::{
            Observed, RecoveryDirective, recover_operation,
        };

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
            recover_operation(&Observed::from_record(
                &committed,
                process.version.clone(),
                0
            ))
            .directive,
            RecoveryDirective::FinalizeCommit,
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
        assert!(
            format!("{outcome:?}").starts_with("Committed"),
            "the finalize must commit, not lose a compare-and-swap: {outcome:?}"
        );
        let loaded = repository
            .load("process")
            .await
            .unwrap()
            .expect("the aggregate exists");
        let operation = loaded.snapshot.operations.get(&operation_id).unwrap();
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

    /// The boot repair pass counts only the aggregates it actually rewrote:
    /// a consistent one is visited and left alone.
    #[tokio::test]
    async fn repair_all_ready_counts_only_rewritten_aggregates() {
        let (first, _) = ready_snapshot("repair-a");
        let (second, _) = ready_snapshot("repair-b");
        let (repository, store) = repository_with_store();
        repository.create(&first, 0, 800).await.unwrap();
        repository.create(&second, 0, 800).await.unwrap();

        assert_eq!(repository.repair_all_ready(0, 800).await.unwrap(), 0);
        store
            .delete(&ready_index_key("repair-a").unwrap())
            .await
            .unwrap();
        assert_eq!(repository.repair_all_ready(0, 800).await.unwrap(), 1);
        assert_eq!(repository.repair_all_ready(0, 800).await.unwrap(), 0);
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
            for candidate in page.operations {
                let operation = candidate.operation;
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

    fn snapshot_for(process_id: &str) -> ProcessSnapshot {
        let mut source = snapshot(true);
        source.process_id = process_id.into();
        source.operations = source
            .operations
            .into_values()
            .map(|mut operation| {
                operation.id.process_id = process_id.into();
                (operation.id.clone(), operation)
            })
            .collect();
        source.outbox = source
            .outbox
            .into_values()
            .map(|mut record| {
                record.id.process_id = process_id.into();
                (record.id.clone(), record)
            })
            .collect();
        source
    }

    /// A 30-task aggregate: enough that a whole-snapshot write dwarfs the
    /// records one commit changes.
    fn long_snapshot(process_id: &str) -> ProcessSnapshot {
        let mut tasks = HashMap::new();
        let mut variables = BTreeMap::new();
        let mut previous = None;
        for index in 0..30 {
            let id = TaskId::new(format!("step{index}"), 0).unwrap();
            tasks.insert(
                id.clone(),
                TaskSnapshot {
                    model_node: NodeId::parse("model-step").unwrap(),
                    parent: None,
                    predecessors: previous.clone().into_iter().collect(),
                    state: TaskState::Ready,
                    revision: 0,
                },
            );
            variables.insert((id.clone(), "payload".into()), "value".into());
            previous = Some(id);
        }
        ProcessSnapshot {
            process_id: process_id.into(),
            version: ProcessVersion {
                graph: 30,
                process: 1,
                operations: 0,
            },
            tasks,
            variables,
            operations: HashMap::new(),
            outbox: HashMap::new(),
        }
    }

    fn legacy_document(source: &ProcessSnapshot, use_v2_payload: bool) -> Vec<u8> {
        if use_v2_payload {
            crate::scheduler::durable::envelope::ProcessEnvelope::seal(
                crate::scheduler::durable::snapshot_codec::SnapshotCodecV2::encode(source).unwrap(),
            )
            .unwrap()
        } else {
            ProcessDocumentCodec::encode(source).unwrap()
        }
    }

    async fn seed_legacy(kv: &crate::store::MemoryStore, source: &ProcessSnapshot, v2: bool) {
        let pid = &source.process_id;
        kv.put(&process_key(pid).unwrap(), legacy_document(source, v2))
            .await
            .unwrap();
        if let Some(index) = recovery_index_document(source).unwrap() {
            kv.put(&recovery_index_key(pid).unwrap(), index)
                .await
                .unwrap();
        }
        if let Some(index) = outbox_index_document(source).unwrap() {
            kv.put(&outbox_index_key(pid).unwrap(), index)
                .await
                .unwrap();
        }
    }

    /// An old V2 (payload) or V3 document opens through the new family: the
    /// load migrates it in one atomic batch, the rebuilt snapshot is
    /// value-equal, and a second load reads the family without rewriting it.
    #[tokio::test]
    async fn legacy_v2_and_v3_documents_migrate_on_load() {
        for (pid, v2) in [("legacy-v2", true), ("legacy-v3", false)] {
            let kv = Arc::new(crate::store::MemoryStore::new());
            let repository =
                AsyncProcessRepository::new(Arc::new(KvDocumentStore::new(kv.clone())));
            let source = snapshot_for(pid);
            seed_legacy(&kv, &source, v2).await;
            let legacy_bytes = kv.one(&process_key(pid).unwrap()).await.unwrap().unwrap();

            let loaded = repository.load(pid).await.unwrap().expect("aggregate");
            assert_eq!(loaded.snapshot, source);
            assert!(!loaded.is_legacy(), "{pid} must migrate on load");
            let head = kv.one(&process_key(pid).unwrap()).await.unwrap().unwrap();
            assert_ne!(head, legacy_bytes, "{pid}'s head is an L8 document");
            assert!(
                kv.one(&process_base_key(pid).unwrap())
                    .await
                    .unwrap()
                    .is_some(),
                "{pid}'s base snapshot was written"
            );

            let again = repository.load(pid).await.unwrap().expect("aggregate");
            assert_eq!(again.snapshot, source);
            assert_eq!(
                again.original_document(),
                head,
                "a migrated aggregate is not rewritten on the next load"
            );
        }
    }

    enum MigrationFault {
        StoreError,
        LostGuard,
    }

    /// Refuses the first migration batch, either with a store error or with a
    /// lost guard.
    struct FaultyMigrationStore {
        inner: Arc<KvDocumentStore>,
        fault: MigrationFault,
        fired: std::sync::atomic::AtomicBool,
    }

    #[async_trait::async_trait]
    impl AsyncDocumentStore for FaultyMigrationStore {
        async fn read_many(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>, ActError> {
            self.inner.read_many(keys).await
        }
        async fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, ActError> {
            self.inner.scan_prefix(prefix).await
        }
        async fn scan_range(
            &self,
            prefix: &str,
            lower: Option<&str>,
            upper: Option<&str>,
        ) -> Result<Vec<(String, Vec<u8>)>, ActError> {
            self.inner.scan_range(prefix, lower, upper).await
        }
        async fn atomic_batch(
            &self,
            guards: &[DocumentGuard],
            mutations: &[DocumentMutation],
        ) -> Result<bool, ActError> {
            if !self.fired.swap(true, Ordering::SeqCst) {
                return match self.fault {
                    MigrationFault::StoreError => {
                        Err(ActError::Store("injected migration failure".into()))
                    }
                    MigrationFault::LostGuard => Ok(false),
                };
            }
            self.inner.atomic_batch(guards, mutations).await
        }
    }

    /// A migration the store refuses leaves the legacy document readable —
    /// the load still answers the old snapshot — and the next load retries
    /// and succeeds. A lost guard re-reads and then migrates too.
    #[tokio::test]
    async fn migration_failure_keeps_the_legacy_document_readable_and_reenters() {
        for (pid, fault) in [
            ("migration-error", MigrationFault::StoreError),
            ("migration-conflict", MigrationFault::LostGuard),
        ] {
            let kv = Arc::new(crate::store::MemoryStore::new());
            let inner = Arc::new(KvDocumentStore::new(kv.clone()));
            let source = snapshot_for(pid);
            seed_legacy(&kv, &source, false).await;
            let legacy_bytes = kv.one(&process_key(pid).unwrap()).await.unwrap().unwrap();
            let repository = AsyncProcessRepository::new(Arc::new(FaultyMigrationStore {
                inner,
                fault,
                fired: std::sync::atomic::AtomicBool::new(false),
            }));

            let first = repository.load(pid).await.unwrap().expect("aggregate");
            assert_eq!(first.snapshot, source);
            let second = repository.load(pid).await.unwrap().expect("aggregate");
            assert_eq!(second.snapshot, source);
            assert!(!second.is_legacy(), "{pid} must migrate on the retry");
            assert_ne!(
                kv.one(&process_key(pid).unwrap()).await.unwrap().unwrap(),
                legacy_bytes,
                "{pid}'s head was rewritten by the retry"
            );
        }
    }

    /// A commit that leaves a derived index (and a per-operation recovery
    /// entry) byte-identical does not rewrite it: the key's exact guard is
    /// still in the batch, but the identical write is left out. Before L5 the
    /// same batch carried a mutation for every derived key on every commit.
    #[tokio::test]
    async fn a_commit_that_leaves_derived_indexes_unchanged_does_not_rewrite_them() {
        use crate::scheduler::durable::snapshot_codec::LogChanges;

        let kv = Arc::new(crate::store::MemoryStore::new());
        let repository = AsyncProcessRepository::new(Arc::new(KvDocumentStore::new(kv)));
        let source = snapshot(true);
        let loaded = repository.create(&source, 0, 800).await.unwrap();

        // Move one task's revision only: the Claimed operation (and therefore
        // the recovery index and its entry) cannot move, and the outbox/ready
        // indexes stay empty.
        let mut replacement = loaded.snapshot.clone();
        replacement.version.process += 1;
        let task = replacement.tasks.keys().next().unwrap().clone();
        replacement.tasks.get_mut(&task).unwrap().revision += 1;
        let mut changes = LogChanges::default();
        changes.tasks.insert(task);

        let entries = recovery_entries(&loaded.snapshot);
        let InstallWrite {
            transaction, write, ..
        } = build_replace_transaction(&loaded, &replacement, &entries, None, &changes).unwrap();
        assert!(
            write.recovery_index.is_some(),
            "the operation is still in recovery"
        );
        assert!(write.outbox_index.is_none());
        assert!(write.ready.is_none());
        let index_mutations: Vec<&str> = transaction
            .mutations()
            .iter()
            .filter_map(|mutation| match mutation {
                crate::scheduler::durable::document_store::DocumentMutation::Put {
                    key, ..
                }
                | crate::scheduler::durable::document_store::DocumentMutation::Delete { key } => {
                    key.starts_with("kernel/indexes/").then_some(key.as_str())
                }
            })
            .collect();
        assert!(
            index_mutations.is_empty(),
            "unchanged derived documents must not be rewritten: {index_mutations:?}"
        );
    }

    /// A normal commit appends one small log entry and never rewrites the
    /// 30-task base; the commit that fills the window compacts the log away in
    /// the same guarded batch.
    #[tokio::test]
    async fn commits_append_log_entries_and_compact_at_the_window() {
        use crate::scheduler::durable::snapshot_codec::LogChanges;

        let kv = Arc::new(crate::store::MemoryStore::new());
        let repository = AsyncProcessRepository::new(Arc::new(KvDocumentStore::new(kv.clone())));
        let process_id = "log-window";
        let source = long_snapshot(process_id);
        let mut loaded = repository.create(&source, 0, 800).await.unwrap();
        let base_key = process_base_key(process_id).unwrap();
        let base_before = kv.one(&base_key).await.unwrap().unwrap();
        let log_prefix = process_log_prefix(process_id).unwrap();
        let mut log_bytes = 0u64;

        for step in 0..MAX_LOG_ENTRIES {
            let mut replacement = loaded.snapshot.clone();
            replacement.version.process += 1;
            let task = replacement.tasks.keys().next().unwrap().clone();
            replacement.tasks.get_mut(&task).unwrap().revision += 1;
            let mut changes = LogChanges::default();
            changes.tasks.insert(task);
            let entries = recovery_entries(&loaded.snapshot);
            let outcome = repository
                .replace(&loaded, &replacement, &entries, None, &changes)
                .await
                .unwrap();
            assert!(
                format!("{outcome:?}").starts_with("Committed"),
                "{outcome:?}"
            );
            assert_eq!(
                kv.one(&base_key).await.unwrap().unwrap(),
                base_before,
                "no ordinary commit rewrites the base"
            );
            loaded = repository
                .load(process_id)
                .await
                .unwrap()
                .expect("aggregate");
            let (_, entry) = loaded
                .original_log()
                .iter()
                .next_back()
                .expect("an ordinary commit appends a log entry");
            assert_eq!(loaded.original_log().len() as u64, step + 1);
            assert!(
                (entry.len() as u64) * 4 < base_before.len() as u64,
                "one commit's log entry ({} B) must not approach the {}-task base ({} B)",
                entry.len(),
                source.tasks.len(),
                base_before.len()
            );
            log_bytes += entry.len() as u64;
        }
        let logged = kv
            .scan_prefix(
                &log_prefix,
                crate::store::ScanOptions::new(
                    crate::store::ScanOperation::Eq,
                    log_prefix.clone(),
                    false,
                ),
            )
            .await
            .unwrap();
        assert_eq!(logged.len() as u64, MAX_LOG_ENTRIES);
        assert_eq!(
            logged
                .iter()
                .map(|(_, bytes)| bytes.len() as u64)
                .sum::<u64>(),
            log_bytes
        );
        assert_eq!(kv.one(&base_key).await.unwrap().unwrap(), base_before);

        let mut replacement = loaded.snapshot.clone();
        replacement.version.process += 1;
        let task = replacement.tasks.keys().next().unwrap().clone();
        replacement.tasks.get_mut(&task).unwrap().revision += 1;
        let mut changes = LogChanges::default();
        changes.tasks.insert(task);
        let entries = recovery_entries(&loaded.snapshot);
        let outcome = repository
            .replace(&loaded, &replacement, &entries, None, &changes)
            .await
            .unwrap();
        assert!(
            format!("{outcome:?}").starts_with("Committed"),
            "{outcome:?}"
        );
        let compacted = repository
            .load(process_id)
            .await
            .unwrap()
            .expect("aggregate");
        assert!(
            compacted.original_log().is_empty(),
            "the full window folds into a base"
        );
        assert_eq!(compacted.base_seq(), compacted.next_seq());
        assert!(
            kv.scan_prefix(
                &log_prefix,
                crate::store::ScanOptions::new(
                    crate::store::ScanOperation::Eq,
                    log_prefix.clone(),
                    false,
                ),
            )
            .await
            .unwrap()
            .is_empty()
        );
        assert_ne!(kv.one(&base_key).await.unwrap().unwrap(), base_before);
    }

    /// Serves hand-crafted scan results for one prefix while every read and
    /// write (and every other prefix) goes to the real store.
    struct StaleScanStore {
        inner: Arc<KvDocumentStore>,
        prefix: String,
        entries: Vec<(String, Vec<u8>)>,
    }

    #[async_trait::async_trait]
    impl AsyncDocumentStore for StaleScanStore {
        async fn read_many(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>, ActError> {
            self.inner.read_many(keys).await
        }
        async fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, ActError> {
            if prefix == self.prefix {
                return Ok(self.entries.clone());
            }
            self.inner.scan_prefix(prefix).await
        }
        async fn scan_range(
            &self,
            prefix: &str,
            lower: Option<&str>,
            upper: Option<&str>,
        ) -> Result<Vec<(String, Vec<u8>)>, ActError> {
            if prefix == self.prefix {
                return Ok(self.entries.clone());
            }
            self.inner.scan_range(prefix, lower, upper).await
        }
        async fn atomic_batch(
            &self,
            guards: &[DocumentGuard],
            mutations: &[DocumentMutation],
        ) -> Result<bool, ActError> {
            self.inner.atomic_batch(guards, mutations).await
        }
    }

    #[derive(Clone, Copy)]
    enum BatchFault {
        Lose,
        LoseAfterApply,
        Fail,
    }

    /// Injects one storage fault: a permanently lost guard, one lost guard
    /// whose mutations still landed (a rival writer won), a failed batch, or a
    /// failed read.
    struct FaultyStore {
        inner: Arc<KvDocumentStore>,
        fault: BatchFault,
        read_fails: bool,
        fired: std::sync::atomic::AtomicBool,
    }

    #[async_trait::async_trait]
    impl AsyncDocumentStore for FaultyStore {
        async fn read_many(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>, ActError> {
            if self.read_fails {
                return Err(ActError::Store("injected read failure".into()));
            }
            self.inner.read_many(keys).await
        }
        async fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, ActError> {
            self.inner.scan_prefix(prefix).await
        }
        async fn scan_range(
            &self,
            prefix: &str,
            lower: Option<&str>,
            upper: Option<&str>,
        ) -> Result<Vec<(String, Vec<u8>)>, ActError> {
            self.inner.scan_range(prefix, lower, upper).await
        }
        async fn atomic_batch(
            &self,
            guards: &[DocumentGuard],
            mutations: &[DocumentMutation],
        ) -> Result<bool, ActError> {
            match self.fault {
                BatchFault::Fail => Err(ActError::Store("injected batch failure".into())),
                BatchFault::Lose => Ok(false),
                BatchFault::LoseAfterApply => {
                    if !self.fired.swap(true, Ordering::SeqCst) {
                        self.inner.atomic_batch(&[], mutations).await?;
                        Ok(false)
                    } else {
                        self.inner.atomic_batch(guards, mutations).await
                    }
                }
            }
        }
    }

    fn faulty_repository(
        inner: Arc<KvDocumentStore>,
        fault: BatchFault,
        read_fails: bool,
    ) -> AsyncProcessRepository {
        AsyncProcessRepository::new(Arc::new(FaultyStore {
            inner,
            fault,
            read_fails,
            fired: std::sync::atomic::AtomicBool::new(false),
        }))
    }

    #[tokio::test]
    async fn create_reports_conflict_when_the_aggregate_already_exists() {
        let (repository, _) = repository_with_store();
        let (process, _) = ready_snapshot("process");
        repository.create(&process, 0, 800).await.unwrap();
        let error = repository.create(&process, 0, 800).await.unwrap_err();
        assert!(error.to_string().contains(": process"));
    }

    #[tokio::test]
    async fn repository_pages_reject_out_of_range_limits() {
        let (repository, _) = repository_with_store();
        let repository = Arc::new(repository);
        let recovery = AsyncRecoveryRepository::new(repository.clone());
        for limit in [0, MAX_RECOVERY_PAGE_SIZE + 1] {
            assert!(matches!(
                recovery.recoverable_page(None, limit).await,
                Err(AsyncRepositoryError::Domain(CommitError::InvalidPageLimit))
            ));
            assert!(matches!(
                repository.due_ready_page(None, 0, limit).await,
                Err(AsyncRepositoryError::Domain(CommitError::InvalidPageLimit))
            ));
        }
        assert_eq!(
            repository
                .dispatchable_outbox_page(0, None, 0)
                .await
                .unwrap_err(),
            OutboxRepositoryError::InvalidPageLimit
        );
    }

    /// A corrupt recovery index is damage; candidates whose aggregate or
    /// operation moved on (or never existed) are stale and skipped.
    #[tokio::test]
    async fn recovery_page_skips_corrupt_and_stale_index_documents() {
        let kv = Arc::new(crate::store::MemoryStore::new());
        let inner = Arc::new(KvDocumentStore::new(kv.clone()));
        let repository = AsyncProcessRepository::new(inner.clone());
        let (process, operation) = ready_snapshot("process");
        repository.create(&process, 0, 800).await.unwrap();

        let corrupt = AsyncProcessRepository::new(Arc::new(StaleScanStore {
            inner: inner.clone(),
            prefix: RECOVERY_INDEX_KEY_PREFIX.into(),
            entries: vec![(recovery_index_key("process").unwrap(), b"junk".to_vec())],
        }));
        let error = corrupt.load_recovery_page(None, 10).await.unwrap_err();
        assert!(format!("{error}").contains("recovery index"));

        let ghost = OperationId {
            process_id: "ghost".into(),
            task: TaskId::new("step", 0).unwrap(),
            sequence: 0,
        };
        let unknown = OperationId {
            process_id: "process".into(),
            task: operation.task.clone(),
            sequence: 9,
        };
        let ghost_index = RecoveryIndex {
            process_id: "ghost".into(),
            entries: vec![
                crate::scheduler::core::recovery::recovery_index::RecoveryIndexEntry {
                    operation_id: ghost,
                    revision: 1,
                },
            ],
        };
        let stale_index = RecoveryIndex {
            process_id: "process".into(),
            entries: vec![
                crate::scheduler::core::recovery::recovery_index::RecoveryIndexEntry {
                    operation_id: operation,
                    revision: 99,
                },
                crate::scheduler::core::recovery::recovery_index::RecoveryIndexEntry {
                    operation_id: unknown,
                    revision: 1,
                },
            ],
        };
        let stale = AsyncProcessRepository::new(Arc::new(StaleScanStore {
            inner,
            prefix: RECOVERY_INDEX_KEY_PREFIX.into(),
            entries: vec![
                (
                    recovery_index_key("ghost").unwrap(),
                    RecoveryIndexCodec::encode(&ghost_index).unwrap(),
                ),
                (
                    recovery_index_key("process").unwrap(),
                    RecoveryIndexCodec::encode(&stale_index).unwrap(),
                ),
            ],
        }));
        let page = stale.load_recovery_page(None, 10).await.unwrap();
        assert!(page.operations.is_empty());
        assert!(page.next.is_none());
    }

    /// The cursor may name only a document (no operation yet) and an entry
    /// that sorts after the cursor's operation is still emitted; both are the
    /// non-skip side of the positional filter. A retired operation whose
    /// revision still matches is stale as well, not damage.
    #[tokio::test]
    async fn recovery_cursor_edges_skip_only_what_it_names() {
        let (mut process, first) = ready_snapshot("cursor");
        let task = first.task.clone();
        let second = OperationId {
            process_id: "cursor".into(),
            task: task.clone(),
            sequence: 1,
        };
        let claim = ClaimToken {
            owner: "runtime-a".into(),
            fence: 1,
            lease_until_ms: 9_000,
            heartbeat_revision: 0,
        };
        for id in [&first, &second] {
            process.operations.insert(
                id.clone(),
                OperationRecord::pending(id.clone(), OperationKind::ExecuteTask)
                    .evolve(
                        crate::scheduler::core::model::operation::OperationEvent::ClaimAndPrepare(
                            claim.clone(),
                        ),
                    )
                    .unwrap(),
            );
        }
        process.version.operations = 2;
        let (repository, _) = repository_with_store();
        repository.create(&process, 0, 800).await.unwrap();

        // No operation in the cursor: the document is scanned from its start.
        let document_only = RecoveryCursor {
            process_key: recovery_index_key("cursor").unwrap(),
            operation: None,
        };
        let page = repository
            .load_recovery_page(Some(&document_only), 10)
            .await
            .unwrap();
        assert_eq!(page.operations.len(), 2);

        // The entry after the cursor's operation is emitted (the `is_gt` arm
        // the skip filter must not swallow).
        let after_first = RecoveryCursor {
            process_key: recovery_index_key("cursor").unwrap(),
            operation: Some(first.clone()),
        };
        let page = repository
            .load_recovery_page(Some(&after_first), 10)
            .await
            .unwrap();
        assert_eq!(page.operations.len(), 1);
        assert_eq!(page.operations[0].operation.id, second);

        // Same revision, but the record retired after the index was scanned.
        let (mut retired_process, retired) = ready_snapshot("retired");
        retired_process.operations.insert(
            retired.clone(),
            OperationRecord {
                id: retired.clone(),
                kind: OperationKind::ExecuteTask,
                state: OperationState::Superseded { by_generation: 1 },
                revision: 0,
            },
        );
        let kv = Arc::new(crate::store::MemoryStore::new());
        let inner = Arc::new(KvDocumentStore::new(kv));
        AsyncProcessRepository::new(inner.clone())
            .create(&retired_process, 0, 800)
            .await
            .unwrap();
        let index = RecoveryIndex {
            process_id: "retired".into(),
            entries: vec![
                crate::scheduler::core::recovery::recovery_index::RecoveryIndexEntry {
                    operation_id: retired,
                    revision: 0,
                },
            ],
        };
        let scanned = AsyncProcessRepository::new(Arc::new(StaleScanStore {
            inner,
            prefix: RECOVERY_INDEX_KEY_PREFIX.into(),
            entries: vec![(
                recovery_index_key("retired").unwrap(),
                RecoveryIndexCodec::encode(&index).unwrap(),
            )],
        }));
        let page = scanned.load_recovery_page(None, 10).await.unwrap();
        assert!(page.operations.is_empty());
    }

    #[tokio::test]
    async fn operation_commits_require_the_process_and_its_identity() {
        let (repository, _) = repository_with_store();
        let missing = OperationId {
            process_id: "missing".into(),
            task: TaskId::new("step", 0).unwrap(),
            sequence: 0,
        };
        let error = repository
            .commit_operation_event(
                &missing,
                0,
                crate::scheduler::core::model::operation::OperationEvent::Complete {
                    commit: crate::scheduler::core::model::operation::CommitToken {
                        process_version: 1,
                        digest: "d".into(),
                    },
                },
                0,
                800,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("missing"));

        let (process, id) = ready_snapshot("process");
        let loaded = repository.create(&process, 0, 800).await.unwrap();
        let foreign = OperationId {
            process_id: "foreign".into(),
            ..id
        };
        let error = repository
            .commit_operation_event_from(
                loaded,
                &foreign,
                0,
                crate::scheduler::core::model::operation::OperationEvent::Complete {
                    commit: crate::scheduler::core::model::operation::CommitToken {
                        process_version: 1,
                        digest: "d".into(),
                    },
                },
                0,
                800,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("foreign"));
    }

    fn claim_event() -> crate::scheduler::core::model::operation::OperationEvent {
        crate::scheduler::core::model::operation::OperationEvent::ClaimAndPrepare(ClaimToken {
            owner: "runtime-a".into(),
            fence: 1,
            lease_until_ms: 9_000,
            heartbeat_revision: 0,
        })
    }

    #[tokio::test]
    async fn operation_event_reports_an_already_committed_or_conflicting_write() {
        let (process, id) = ready_snapshot("process");
        let kv = Arc::new(crate::store::MemoryStore::new());
        let inner = Arc::new(KvDocumentStore::new(kv));
        let creator = AsyncProcessRepository::new(inner.clone());
        creator.create(&process, 0, 800).await.unwrap();

        // The guard was lost but the same state landed: the operation is
        // authoritative and the outcome is `AlreadyCommitted`.
        let applied = faulty_repository(inner.clone(), BatchFault::LoseAfterApply, false);
        let loaded = applied.load("process").await.unwrap().expect("aggregate");
        let outcome = applied
            .commit_operation_event_from(loaded, &id, 0, claim_event(), 0, 800)
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            OperationCommitOutcome::AlreadyCommitted { .. }
        ));

        // A second, independent aggregate for the plain lost-guard case.
        let (process, id) = ready_snapshot("process");
        let kv = Arc::new(crate::store::MemoryStore::new());
        let inner = Arc::new(KvDocumentStore::new(kv));
        let creator = AsyncProcessRepository::new(inner.clone());
        let loaded = creator.create(&process, 0, 800).await.unwrap();
        let losing = faulty_repository(inner, BatchFault::Lose, false);
        assert!(matches!(
            losing
                .commit_operation_event_from(loaded, &id, 0, claim_event(), 0, 800)
                .await
                .unwrap(),
            OperationCommitOutcome::Conflict {}
        ));
    }

    fn transition_plan(
        process: &ProcessSnapshot,
        task: TaskId,
    ) -> crate::scheduler::core::model::transition_plan::TransitionPlan {
        use crate::scheduler::core::model::transition_plan::{StateChange, TransitionPlan};
        TransitionPlan {
            process_id: process.process_id.clone(),
            expected: process.version.clone(),
            task_creates: Vec::new(),
            state_changes: vec![StateChange {
                task,
                expected_revision: 1,
                from: TaskState::Ready,
                to: TaskState::Running,
            }],
            variable_writes: Vec::new(),
            source_operation: None,
            operations: Vec::new(),
            outbox: Vec::new(),
        }
    }

    #[tokio::test]
    async fn transition_commit_reports_an_already_committed_or_conflicting_write() {
        let source = snapshot(false);
        let task = source.tasks.keys().next().unwrap().clone();
        let plan = transition_plan(&source, task);
        let kv = Arc::new(crate::store::MemoryStore::new());
        let inner = Arc::new(KvDocumentStore::new(kv));
        let creator = AsyncProcessRepository::new(inner.clone());
        creator.create(&source, 0, 800).await.unwrap();

        let applied = faulty_repository(inner.clone(), BatchFault::LoseAfterApply, false);
        let outcome = applied.commit_transition(&plan, 0, 800).await.unwrap();
        assert!(matches!(
            outcome,
            TransitionCommitOutcome::AlreadyCommitted { .. }
        ));

        // A fresh aggregate for the plain lost-guard case: the applied attempt
        // above advanced the version this plan expects.
        let kv = Arc::new(crate::store::MemoryStore::new());
        let inner = Arc::new(KvDocumentStore::new(kv));
        let creator = AsyncProcessRepository::new(inner.clone());
        creator.create(&source, 0, 800).await.unwrap();
        let losing = faulty_repository(inner, BatchFault::Lose, false);
        assert!(matches!(
            losing.commit_transition(&plan, 0, 800).await.unwrap(),
            TransitionCommitOutcome::Conflict {}
        ));
    }

    /// Two due operations with the same ready time page one at a time: the
    /// cursor skips the entry it names and resumes after it.
    #[tokio::test]
    async fn ready_pages_resume_after_the_cursor() {
        let (mut process, first) = ready_snapshot("process");
        let task = first.task.clone();
        let second = OperationId {
            process_id: "process".into(),
            task: task.clone(),
            sequence: 1,
        };
        process.operations.insert(
            second.clone(),
            OperationRecord::pending(second.clone(), OperationKind::ExecuteTask),
        );
        process.version.operations = 2;
        let (repository, _) = repository_with_store();
        repository.create(&process, 0, 800).await.unwrap();

        let page = repository.due_ready_page(None, 0, 1).await.unwrap();
        assert_eq!(page.observed, 2);
        assert_eq!(page.oldest_ready_at_ms, Some(0));
        assert_eq!(page.operations, vec![first.clone()]);
        let cursor = page.next.expect("a second due operation remains");

        let page = repository
            .due_ready_page(Some(&cursor), 0, 1)
            .await
            .unwrap();
        assert_eq!(page.operations, vec![second]);
        assert!(page.next.is_none());
    }

    /// The ready cursor skips only entries at or before the cursor position:
    /// an earlier ready time in the same document is skipped, a later one is
    /// walked, and a different document after it is never compared with the
    /// cursor at all.
    #[tokio::test]
    async fn ready_cursor_walks_earlier_and_later_entries() {
        let (mut process, first) = ready_snapshot("cursor");
        let task = first.task.clone();
        let second = OperationId {
            process_id: "cursor".into(),
            task: task.clone(),
            sequence: 1,
        };
        process.operations.insert(
            second.clone(),
            OperationRecord::pending(second.clone(), OperationKind::ExecuteTask),
        );
        process.version.operations = 2;
        let (repository, _) = repository_with_store();
        repository.create(&process, 0, 800).await.unwrap();
        repository.set_ready_at(&second, 5).await.unwrap();

        let (other, other_id) = ready_snapshot("zulu");
        repository.create(&other, 0, 800).await.unwrap();

        // An earlier ready time than the cursor is skipped; the other process
        // document is scanned without consulting the cursor's position.
        let cursor = ReadyCursor {
            process_key: ready_index_key("cursor").unwrap(),
            ready_at_ms: 2,
            operation: second.clone(),
        };
        let page = repository
            .due_ready_page(Some(&cursor), 0, 10)
            .await
            .unwrap();
        assert_eq!(page.observed, 2, "only the later entries are walked");
        assert_eq!(page.operations, vec![other_id]);

        // The same position (equal ready time, equal operation) is skipped and
        // a later ready time in the same document is not.
        let equal = ReadyCursor {
            process_key: ready_index_key("cursor").unwrap(),
            ready_at_ms: 0,
            operation: first,
        };
        let page = repository
            .due_ready_page(Some(&equal), 0, 10)
            .await
            .unwrap();
        assert_eq!(page.observed, 2);
    }

    #[tokio::test]
    async fn set_ready_at_handles_missing_stale_and_repeated_edits() {
        let (process, first) = ready_snapshot("process");
        let task = first.task.clone();
        let second = OperationId {
            process_id: "process".into(),
            task,
            sequence: 1,
        };
        let mut process = process;
        process.operations.insert(
            second.clone(),
            OperationRecord::pending(second.clone(), OperationKind::ExecuteTask),
        );
        process.version.operations = 2;
        let (repository, store) = repository_with_store();
        repository.create(&process, 0, 800).await.unwrap();

        // A process with no ready document, and an operation absent from an
        // existing one, are both no-ops.
        assert!(
            !repository
                .set_ready_at(
                    &OperationId {
                        process_id: "absent".into(),
                        task: first.task.clone(),
                        sequence: 0,
                    },
                    10
                )
                .await
                .unwrap()
        );
        let absent_entry = OperationId {
            process_id: "process".into(),
            task: first.task.clone(),
            sequence: 9,
        };
        assert!(!repository.set_ready_at(&absent_entry, 10).await.unwrap());
        assert!(!repository.set_ready_at(&first, 0).await.unwrap());
        assert!(repository.set_ready_at(&second, 5).await.unwrap());

        let corrupt_key = ready_index_key("corrupt").unwrap();
        store.put(&corrupt_key, b"junk".to_vec()).await.unwrap();
        let error = repository
            .set_ready_at(
                &OperationId {
                    process_id: "corrupt".into(),
                    task: first.task.clone(),
                    sequence: 0,
                },
                5,
            )
            .await
            .unwrap_err();
        assert!(matches!(error, AsyncRepositoryError::Domain(_)));

        let inner = Arc::new(KvDocumentStore::new(store.clone()));
        let losing = faulty_repository(inner.clone(), BatchFault::Lose, false);
        assert!(!losing.set_ready_at(&first, 77).await.unwrap());

        let failing = faulty_repository(inner, BatchFault::Lose, true);
        assert!(matches!(
            failing.set_ready_at(&first, 77).await,
            Err(AsyncRepositoryError::Store(_))
        ));
    }

    #[tokio::test]
    async fn repair_ready_is_a_no_op_for_absent_or_unwritable_aggregates() {
        let (repository, _) = repository_with_store();
        assert!(!repository.repair_ready("absent", 0, 800).await.unwrap());

        let (process, _) = ready_snapshot("process");
        let kv = Arc::new(crate::store::MemoryStore::new());
        let inner = Arc::new(KvDocumentStore::new(kv.clone()));
        let creator = AsyncProcessRepository::new(inner.clone());
        creator.create(&process, 0, 800).await.unwrap();
        kv.delete(&ready_index_key("process").unwrap())
            .await
            .unwrap();
        let losing = faulty_repository(inner, BatchFault::Lose, false);
        assert!(!losing.repair_ready("process", 0, 800).await.unwrap());
    }

    #[tokio::test]
    async fn a_corrupt_ready_document_fails_the_commit_that_reads_it() {
        let (process, id) = ready_snapshot("process");
        let kv = Arc::new(crate::store::MemoryStore::new());
        let repository = AsyncProcessRepository::new(Arc::new(KvDocumentStore::new(kv.clone())));
        repository.create(&process, 0, 800).await.unwrap();
        kv.put(&ready_index_key("process").unwrap(), b"junk".to_vec())
            .await
            .unwrap();
        let error = repository
            .commit_operation_event(&id, 0, claim_event(), 0, 800)
            .await
            .unwrap_err();
        assert!(format!("{error}").contains("ready index"));
    }

    fn outbox_snapshot() -> (ProcessSnapshot, OperationId) {
        let mut process = ProcessSnapshot::empty("outbox").unwrap();
        process.tasks.insert(
            TaskId::new("step", 0).unwrap(),
            TaskSnapshot {
                model_node: NodeId::parse("model-step").unwrap(),
                parent: None,
                predecessors: vec![],
                state: TaskState::Completed,
                revision: 1,
            },
        );
        let claim = crate::scheduler::core::outbox::records::DeliveryClaim {
            owner: "owner".into(),
            fence: 1,
            lease_until_ms: 1_000,
            heartbeat_revision: 0,
        };
        let make = |key: &str| crate::scheduler::core::outbox::records::OutboxId {
            process_id: "outbox".into(),
            key: key.into(),
        };
        let pending = crate::scheduler::core::outbox::records::OutboxRecord::pending(
            make("a-pending"),
            "topic".into(),
            "{}".into(),
            0,
        )
        .unwrap();
        let claimed = crate::scheduler::core::outbox::records::OutboxRecord::pending(
            make("b-claimed"),
            "topic".into(),
            "{}".into(),
            0,
        )
        .unwrap()
        .evolve(
            crate::scheduler::core::outbox::records::OutboxEvent::Claim {
                claim: claim.clone(),
                now_ms: 0,
            },
        )
        .unwrap();
        let unknown = crate::scheduler::core::outbox::records::OutboxRecord::pending(
            make("c-unknown"),
            "topic".into(),
            "{}".into(),
            0,
        )
        .unwrap()
        .evolve(
            crate::scheduler::core::outbox::records::OutboxEvent::Claim {
                claim: claim.clone(),
                now_ms: 0,
            },
        )
        .unwrap()
        .evolve(
            crate::scheduler::core::outbox::records::OutboxEvent::MarkDeliveryUnknown {
                claim,
                reason: "unknown".into(),
                now_ms: 0,
            },
        )
        .unwrap();
        for record in [pending, claimed, unknown] {
            process.outbox.insert(record.id.clone(), record);
        }
        let id = OperationId {
            process_id: "outbox".into(),
            task: TaskId::new("step", 0).unwrap(),
            sequence: 0,
        };
        (process, id)
    }

    #[tokio::test]
    async fn outbox_page_orders_due_states_and_resumes_at_its_cursor() {
        let (process, _) = outbox_snapshot();
        let (repository, store) = repository_with_store();
        repository.create(&process, 0, 800).await.unwrap();

        let page = repository
            .dispatchable_outbox_page(0, None, 10)
            .await
            .unwrap();
        assert_eq!(page.observed, 3);
        assert_eq!(page.due, 1);
        assert_eq!(page.records.len(), 1);

        let page = repository
            .dispatchable_outbox_page(2_000, None, 1)
            .await
            .unwrap();
        assert_eq!(page.due, 2);
        assert_eq!(page.records.len(), 1);
        let cursor = page.next.expect("a second due record remains");
        let page = repository
            .dispatchable_outbox_page(2_000, Some(&cursor), 1)
            .await
            .unwrap();
        assert_eq!(page.records.len(), 1);
        assert!(page.records[0].id > cursor.0);

        let corrupt_key = format!("{OUTBOX_INDEX_KEY_PREFIX}corrupt");
        store.put(&corrupt_key, b"junk".to_vec()).await.unwrap();
        let error = repository
            .dispatchable_outbox_page(0, None, 10)
            .await
            .unwrap_err();
        assert!(matches!(error, OutboxRepositoryError::CorruptData(_)));
    }

    /// A stale index entry (an aggregate that moved on after the scan) is
    /// damage the page refuses rather than handing out an outdated record.
    #[tokio::test]
    async fn outbox_page_refuses_a_stale_index_entry() {
        use crate::scheduler::core::outbox::outbox_index::{OutboxIndex, OutboxIndexState};

        let (process, _) = outbox_snapshot();
        let kv = Arc::new(crate::store::MemoryStore::new());
        let inner = Arc::new(KvDocumentStore::new(kv));
        let repository = AsyncProcessRepository::new(inner.clone());
        repository.create(&process, 0, 800).await.unwrap();
        let entry = crate::scheduler::core::outbox::outbox_index::OutboxIndexEntry {
            id: crate::scheduler::core::outbox::records::OutboxId {
                process_id: "outbox".into(),
                key: "a-pending".into(),
            },
            revision: 99,
            state: OutboxIndexState::Pending { available_at_ms: 0 },
        };
        let stale = OutboxIndex {
            process_id: "outbox".into(),
            entries: vec![entry],
        };
        let scanned = AsyncProcessRepository::new(Arc::new(StaleScanStore {
            inner: inner.clone(),
            prefix: OUTBOX_INDEX_KEY_PREFIX.into(),
            entries: vec![(
                outbox_index_key("outbox").unwrap(),
                OutboxIndexCodec::encode(&stale).unwrap(),
            )],
        }));
        let error = scanned
            .dispatchable_outbox_page(0, None, 10)
            .await
            .unwrap_err();
        assert!(matches!(error, OutboxRepositoryError::CorruptData(_)));

        // The revision still matches, but the record's own state says it is
        // not dispatchable right now (a live claim): also stale, not damage.
        let claimed_id = crate::scheduler::core::outbox::records::OutboxId {
            process_id: "outbox".into(),
            key: "b-claimed".into(),
        };
        let claimed = process.outbox.get(&claimed_id).unwrap().clone();
        let not_ready = OutboxIndex {
            process_id: "outbox".into(),
            entries: vec![
                crate::scheduler::core::outbox::outbox_index::OutboxIndexEntry {
                    id: claimed_id,
                    revision: claimed.revision,
                    state: OutboxIndexState::Pending { available_at_ms: 0 },
                },
            ],
        };
        let scanned = AsyncProcessRepository::new(Arc::new(StaleScanStore {
            inner: inner.clone(),
            prefix: OUTBOX_INDEX_KEY_PREFIX.into(),
            entries: vec![(
                outbox_index_key("outbox").unwrap(),
                OutboxIndexCodec::encode(&not_ready).unwrap(),
            )],
        }));
        let error = scanned
            .dispatchable_outbox_page(0, None, 10)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("stale outbox index"));

        // An index entry naming a record the aggregate does not have is the
        // same kind of damage.
        let missing = OutboxIndex {
            process_id: "outbox".into(),
            entries: vec![
                crate::scheduler::core::outbox::outbox_index::OutboxIndexEntry {
                    id: crate::scheduler::core::outbox::records::OutboxId {
                        process_id: "outbox".into(),
                        key: "z-missing".into(),
                    },
                    revision: 0,
                    state: OutboxIndexState::Pending { available_at_ms: 0 },
                },
            ],
        };
        let scanned = AsyncProcessRepository::new(Arc::new(StaleScanStore {
            inner,
            prefix: OUTBOX_INDEX_KEY_PREFIX.into(),
            entries: vec![(
                outbox_index_key("outbox").unwrap(),
                OutboxIndexCodec::encode(&missing).unwrap(),
            )],
        }));
        let error = scanned
            .dispatchable_outbox_page(0, None, 10)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("missing record"));
    }

    #[tokio::test]
    async fn evolve_outbox_reports_revision_conflicts_and_store_failures() {
        let (process, _) = outbox_snapshot();
        let kv = Arc::new(crate::store::MemoryStore::new());
        let inner = Arc::new(KvDocumentStore::new(kv));
        let creator = AsyncProcessRepository::new(inner.clone());
        creator.create(&process, 0, 800).await.unwrap();
        let id = crate::scheduler::core::outbox::records::OutboxId {
            process_id: "outbox".into(),
            key: "a-pending".into(),
        };
        let claim = crate::scheduler::core::outbox::records::DeliveryClaim {
            owner: "owner".into(),
            fence: 1,
            lease_until_ms: 1_000,
            heartbeat_revision: 0,
        };
        let event =
            crate::scheduler::core::outbox::records::OutboxEvent::Claim { claim, now_ms: 0 };
        assert!(matches!(
            creator.evolve_outbox(&id, 99, event.clone()).await,
            Err(OutboxRepositoryError::RevisionConflict { .. })
        ));
        assert!(matches!(
            faulty_repository(inner.clone(), BatchFault::Lose, false)
                .evolve_outbox(&id, 0, event.clone())
                .await,
            Err(OutboxRepositoryError::ConcurrentModification(_))
        ));
        assert!(matches!(
            faulty_repository(inner, BatchFault::Fail, false)
                .evolve_outbox(&id, 0, event)
                .await,
            Err(OutboxRepositoryError::Unavailable(_))
        ));
    }

    #[tokio::test]
    async fn confirm_outbox_batch_skips_inapplicable_records_and_restores_on_failure() {
        use crate::scheduler::core::outbox::outbox_repository::OutboxConfirmation;
        use crate::scheduler::core::outbox::records::{DeliveryClaim, OutboxEvent, OutboxId};

        async fn seeded() -> (
            AsyncProcessRepository,
            Arc<KvDocumentStore>,
            LoadedProcess,
            OutboxId,
        ) {
            let (process, _) = outbox_snapshot();
            let kv = Arc::new(crate::store::MemoryStore::new());
            let inner = Arc::new(KvDocumentStore::new(kv));
            let creator = AsyncProcessRepository::new(inner.clone());
            let loaded = creator.create(&process, 0, 800).await.unwrap();
            let id = OutboxId {
                process_id: "outbox".into(),
                key: "a-pending".into(),
            };
            (creator, inner, loaded, id)
        }

        let claim = DeliveryClaim {
            owner: "owner".into(),
            fence: 1,
            lease_until_ms: 1_000,
            heartbeat_revision: 0,
        };
        let events = vec![
            OutboxEvent::Claim {
                claim: claim.clone(),
                now_ms: 0,
            },
            OutboxEvent::ConfirmDelivered {
                claim,
                receipt: "ok".into(),
                now_ms: 0,
            },
        ];

        let (creator, inner, mut loaded, id) = seeded().await;
        assert!(
            creator
                .confirm_outbox_batch(&mut loaded, &[])
                .await
                .unwrap()
                .confirmed
                .is_empty()
        );
        for confirmation in [
            OutboxConfirmation {
                id: OutboxId {
                    process_id: "outbox".into(),
                    key: "missing".into(),
                },
                expected_revision: 0,
                events: vec![],
            },
            OutboxConfirmation {
                id: id.clone(),
                expected_revision: 99,
                events: vec![],
            },
            OutboxConfirmation {
                id: id.clone(),
                expected_revision: 0,
                events: vec![OutboxEvent::Quarantine {
                    reason: "  ".into(),
                }],
            },
        ] {
            assert!(
                creator
                    .confirm_outbox_batch(&mut loaded, &[confirmation])
                    .await
                    .unwrap()
                    .confirmed
                    .is_empty()
            );
        }

        // The ordinary path installs the written bytes.
        let outcome = creator
            .confirm_outbox_batch(
                &mut loaded,
                &[OutboxConfirmation {
                    id: id.clone(),
                    expected_revision: 0,
                    events: events.clone(),
                }],
            )
            .await
            .unwrap();
        assert_eq!(outcome.confirmed.len(), 1);
        assert!(loaded.snapshot.outbox[&id].state.is_terminal());
        let index_bytes = inner
            .read_many(&[outbox_index_key("outbox").unwrap()])
            .await
            .unwrap()[0]
            .clone()
            .expect("the other records still project an index");
        let index =
            crate::scheduler::core::outbox::outbox_index::OutboxIndexCodec::decode(&index_bytes)
                .unwrap();
        assert!(index.entries.iter().all(|entry| entry.id != id));

        // A store failure restores the caller's snapshot: the record is still
        // pending and can be redelivered.
        let (_, inner, mut failing_loaded, id) = seeded().await;
        let failing = faulty_repository(inner, BatchFault::Fail, false);
        assert!(matches!(
            failing
                .confirm_outbox_batch(
                    &mut failing_loaded,
                    &[OutboxConfirmation {
                        id: id.clone(),
                        expected_revision: 0,
                        events: events.clone(),
                    }]
                )
                .await,
            Err(OutboxRepositoryError::Unavailable(_))
        ));
        assert_eq!(failing_loaded.snapshot.outbox.get(&id).unwrap().revision, 0);

        // The guard is lost but the applied bytes are authoritative: the
        // confirmation is adopted rather than replayed.
        let (_, inner, _, id) = seeded().await;
        let applied = faulty_repository(inner, BatchFault::LoseAfterApply, false);
        let mut applied_loaded = applied.load("outbox").await.unwrap().expect("aggregate");
        let outcome = applied
            .confirm_outbox_batch(
                &mut applied_loaded,
                &[OutboxConfirmation {
                    id,
                    expected_revision: 0,
                    events,
                }],
            )
            .await
            .unwrap();
        assert_eq!(outcome.confirmed.len(), 1);
    }

    #[tokio::test]
    async fn reclaim_reports_store_failures_and_exhausted_conflicts() {
        let process = reclaimable_snapshot("process");
        let kv = Arc::new(crate::store::MemoryStore::new());
        let inner = Arc::new(KvDocumentStore::new(kv.clone()));
        let creator = AsyncProcessRepository::new(inner.clone());
        creator.create(&process, 0, 800).await.unwrap();
        assert!(matches!(
            faulty_repository(inner.clone(), BatchFault::Fail, false)
                .reclaim("process")
                .await,
            Err(AsyncRepositoryError::Store(_))
        ));
        let error = faulty_repository(inner.clone(), BatchFault::Lose, false)
            .reclaim("process")
            .await
            .unwrap_err();
        assert!(error.to_string().contains(": process"));

        // The orphan path fails the same way, and a permanently lost guard
        // exhausts its attempts too.
        kv.put(&process_base_key("orphan").unwrap(), b"base".to_vec())
            .await
            .unwrap();
        assert!(matches!(
            faulty_repository(inner.clone(), BatchFault::Fail, false)
                .reclaim("orphan")
                .await,
            Err(AsyncRepositoryError::Store(_))
        ));
        let error = faulty_repository(inner, BatchFault::Lose, false)
            .reclaim("orphan")
            .await
            .unwrap_err();
        assert!(error.to_string().contains(": orphan"));
    }

    #[tokio::test]
    async fn a_lost_migration_guard_for_every_attempt_is_a_conflict() {
        let kv = Arc::new(crate::store::MemoryStore::new());
        let inner = Arc::new(KvDocumentStore::new(kv.clone()));
        let source = snapshot_for("legacy-conflict");
        seed_legacy(&kv, &source, false).await;
        let repository = faulty_repository(inner, BatchFault::Lose, false);
        let error = repository.load("legacy-conflict").await.unwrap_err();
        assert!(error.to_string().contains("legacy-conflict"));
    }

    /// A head that moved while its log was damaged is re-read; the retry stops
    /// at the bounded attempt count and surfaces the damage.
    struct MovedHeadStore {
        head: Vec<u8>,
        moved: Vec<u8>,
        base: Vec<u8>,
        log_key: String,
        bad_log: Vec<u8>,
    }

    #[async_trait::async_trait]
    impl AsyncDocumentStore for MovedHeadStore {
        async fn read_many(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>, ActError> {
            if keys.len() == 1 {
                return Ok(vec![Some(self.moved.clone())]);
            }
            Ok(vec![
                Some(self.head.clone()),
                None,
                None,
                None,
                Some(self.base.clone()),
            ])
        }
        async fn scan_prefix(&self, _prefix: &str) -> Result<Vec<(String, Vec<u8>)>, ActError> {
            Ok(vec![(self.log_key.clone(), self.bad_log.clone())])
        }
        async fn scan_range(
            &self,
            _prefix: &str,
            _lower: Option<&str>,
            _upper: Option<&str>,
        ) -> Result<Vec<(String, Vec<u8>)>, ActError> {
            Ok(Vec::new())
        }
        async fn atomic_batch(
            &self,
            _guards: &[DocumentGuard],
            _mutations: &[DocumentMutation],
        ) -> Result<bool, ActError> {
            Ok(false)
        }
    }

    #[tokio::test]
    async fn load_retries_a_damaged_log_while_the_head_moves() {
        use crate::scheduler::durable::document::ProcessLogCodec;
        use crate::scheduler::durable::snapshot_codec::HeadPayload;

        let source = snapshot(false);
        let head = ProcessLogCodec::encode_head(&HeadPayload {
            process_id: source.process_id.clone(),
            version: source.version.clone(),
            epoch: 1,
            base_seq: 0,
            next_seq: 1,
        })
        .unwrap();
        let moved = ProcessLogCodec::encode_head(&HeadPayload {
            process_id: source.process_id.clone(),
            version: source.version.clone(),
            epoch: 2,
            base_seq: 0,
            next_seq: 0,
        })
        .unwrap();
        let base = ProcessLogCodec::encode_base(&source, 0).unwrap();
        let repository = AsyncProcessRepository::new(Arc::new(MovedHeadStore {
            head,
            moved,
            base,
            log_key: process_log_prefix("process").unwrap() + "00000000000000000000",
            bad_log: b"not an envelope".to_vec(),
        }));
        let error = repository.load("process").await.unwrap_err();
        assert!(matches!(error, AsyncRepositoryError::Integrity(_)));
    }

    #[tokio::test]
    async fn async_error_display_names_the_failure_class() {
        assert!(
            AsyncRepositoryError::Conflict("p".into())
                .to_string()
                .contains("already exists or changed: p")
        );
    }

    /// A legacy database may already carry an outbox index; seeding a fixture
    /// with one exercises the same migration/load path.
    #[tokio::test]
    async fn legacy_documents_with_an_outbox_index_still_migrate() {
        let kv = Arc::new(crate::store::MemoryStore::new());
        let repository = AsyncProcessRepository::new(Arc::new(KvDocumentStore::new(kv.clone())));
        let pid = "legacy-outbox";
        let mut source = snapshot(false);
        source.process_id = pid.into();
        let id = crate::scheduler::core::outbox::records::OutboxId {
            process_id: pid.into(),
            key: "k".into(),
        };
        source.outbox.insert(
            id.clone(),
            crate::scheduler::core::outbox::records::OutboxRecord::pending(
                id,
                "topic".into(),
                "{}".into(),
                0,
            )
            .unwrap(),
        );
        seed_legacy(&kv, &source, false).await;
        assert!(
            kv.one(&outbox_index_key(pid).unwrap())
                .await
                .unwrap()
                .is_some()
        );
        let loaded = repository.load(pid).await.unwrap().expect("aggregate");
        assert_eq!(loaded.snapshot, source);
        assert!(!loaded.is_legacy());
    }

    /// A base document names the log sequence it covers. Pairing it with a head
    /// from another window is a torn read, not a silently mixed snapshot.
    #[tokio::test]
    async fn a_base_from_another_window_is_not_mixed_into_the_snapshot() {
        use crate::scheduler::durable::document::ProcessLogCodec;

        let kv = Arc::new(crate::store::MemoryStore::new());
        let repository = AsyncProcessRepository::new(Arc::new(KvDocumentStore::new(kv.clone())));
        let source = snapshot(false);
        repository.create(&source, 0, 800).await.unwrap();
        kv.put(
            &process_base_key("process").unwrap(),
            ProcessLogCodec::encode_base(&source, 7).unwrap(),
        )
        .await
        .unwrap();

        let error = repository.load("process").await.unwrap_err();
        assert!(
            format!("{error}").contains("base covers sequence 7"),
            "the mismatched base must be surfaced, got {error}"
        );
    }
}
