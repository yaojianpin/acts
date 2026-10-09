use super::commit::{
    AggregateRepository, CommitError, CommitReceipt, OperationRepository, RecoveryCursor,
    RecoveryPage, RecoveryRepository, apply_operation_event, apply_transition,
};
use super::document::{AggregateDocumentCodec, AggregateDocumentError};
use super::outbox::{OutboxEvent, OutboxId, OutboxRecord};
use super::outbox_index::{OutboxIndex, OutboxIndexCodec, OutboxIndexState};
use super::outbox_repository::{OutboxCursor, OutboxPage, OutboxRepository, OutboxRepositoryError};
use super::recovery_entry::RecoveryEntryDocument;
use super::recovery_index::{RecoveryIndex, RecoveryIndexCodec};
use super::{AggregateSnapshot, TransitionPlan};
use std::collections::HashMap;
use std::fmt;

const AGGREGATE_KEY_PREFIX: &str = "kernel/aggregates/";
pub(super) const RECOVERY_INDEX_KEY_PREFIX: &str = "kernel/indexes/recovery/";
const OUTBOX_INDEX_KEY_PREFIX: &str = "kernel/indexes/outbox/";

/// A loaded aggregate and the exact bytes observed from storage. The raw bytes
/// are the CAS token. Re-encoding the snapshot is forbidden because a future
/// codec may normalize legacy documents into different bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LoadedAggregate {
    pub(crate) snapshot: AggregateSnapshot,
    original_document: Vec<u8>,
    original_recovery_index: Option<Vec<u8>>,
    original_outbox_index: Option<Vec<u8>>,
}

impl LoadedAggregate {
    pub(crate) fn original_document(&self) -> &[u8] {
        &self.original_document
    }
    pub(crate) fn original_recovery_index(&self) -> Option<&[u8]> {
        self.original_recovery_index.as_deref()
    }
    pub(crate) fn original_outbox_index(&self) -> Option<&[u8]> {
        self.original_outbox_index.as_deref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DocumentGuard {
    pub(crate) key: String,
    pub(crate) expected: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DocumentMutation {
    Put { key: String, value: Vec<u8> },
    Delete { key: String },
}

/// Complete, validated installation intent for one atomic storage commit.
/// Construction rejects ambiguous duplicate keys before any backend is called.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct DocumentTransaction {
    guards: Vec<DocumentGuard>,
    mutations: Vec<DocumentMutation>,
}

impl DocumentTransaction {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn require_absent(
        mut self,
        key: impl Into<String>,
    ) -> Result<Self, DocumentStoreError> {
        self.guards.push(DocumentGuard {
            key: key.into(),
            expected: None,
        });
        validate_document_transaction_structure(&self.guards, &self.mutations)?;
        Ok(self)
    }

    pub(crate) fn require_exact(
        mut self,
        key: impl Into<String>,
        expected: impl Into<Vec<u8>>,
    ) -> Result<Self, DocumentStoreError> {
        self.guards.push(DocumentGuard {
            key: key.into(),
            expected: Some(expected.into()),
        });
        validate_document_transaction_structure(&self.guards, &self.mutations)?;
        Ok(self)
    }

    pub(crate) fn put(
        mut self,
        key: impl Into<String>,
        value: impl Into<Vec<u8>>,
    ) -> Result<Self, DocumentStoreError> {
        self.mutations.push(DocumentMutation::Put {
            key: key.into(),
            value: value.into(),
        });
        validate_document_transaction_structure(&self.guards, &self.mutations)?;
        Ok(self)
    }

    pub(crate) fn delete(mut self, key: impl Into<String>) -> Result<Self, DocumentStoreError> {
        self.mutations
            .push(DocumentMutation::Delete { key: key.into() });
        validate_document_transaction_structure(&self.guards, &self.mutations)?;
        Ok(self)
    }

    pub(crate) fn guards(&self) -> &[DocumentGuard] {
        &self.guards
    }
    pub(crate) fn mutations(&self) -> &[DocumentMutation] {
        &self.mutations
    }

    pub(crate) fn validate(&self) -> Result<(), DocumentStoreError> {
        validate_atomic_batch(&self.guards, &self.mutations)
    }
}

pub(crate) trait AtomicDocumentStore {
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, DocumentStoreError>;

    /// Return a stable key-ordered snapshot of documents under `prefix`.
    /// Implementations must not expose backend iteration order.
    fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, DocumentStoreError>;

    /// Apply all mutations as one atomic transaction if every guard matches.
    /// A false result guarantees that no mutation became visible.
    fn atomic_batch(
        &mut self,
        guards: &[DocumentGuard],
        mutations: &[DocumentMutation],
    ) -> Result<bool, DocumentStoreError>;
}

/// Storage repository concerned only with document integrity and raw-byte CAS.
/// Domain planning and TransitionPlan application remain outside this layer.
pub(crate) struct AggregateDocumentRepository<S> {
    store: S,
}

impl<S: AtomicDocumentStore> AggregateDocumentRepository<S> {
    pub(crate) fn new(store: S) -> Self {
        Self { store }
    }

    pub(crate) fn load(
        &self,
        process_id: &str,
    ) -> Result<Option<LoadedAggregate>, DocumentRepositoryError> {
        let Some(original_document) = self.store.read(&aggregate_key(process_id)?)? else {
            return Ok(None);
        };
        let original_recovery_index = self.store.read(&recovery_index_key(process_id)?)?;
        let original_outbox_index = self.store.read(&outbox_index_key(process_id)?)?;
        decode_loaded_aggregate(
            process_id,
            original_document,
            original_recovery_index,
            original_outbox_index,
        )
        .map(Some)
    }

    pub(crate) fn create(
        &mut self,
        snapshot: &AggregateSnapshot,
    ) -> Result<LoadedAggregate, DocumentRepositoryError> {
        let (transaction, document, recovery_index, outbox_index) =
            build_create_transaction(snapshot)?;
        if !execute_transaction(&mut self.store, &transaction)? {
            return Err(DocumentRepositoryError::Conflict(
                snapshot.process_id.clone(),
            ));
        }
        Ok(loaded_after_write(
            snapshot,
            document,
            recovery_index,
            outbox_index,
        ))
    }

    pub(crate) fn replace(
        &mut self,
        loaded: &LoadedAggregate,
        replacement_snapshot: &AggregateSnapshot,
    ) -> Result<LoadedAggregate, DocumentRepositoryError> {
        let (transaction, document, recovery_index, outbox_index) =
            build_replace_transaction(loaded, replacement_snapshot)?;
        if !execute_transaction(&mut self.store, &transaction)? {
            return Err(DocumentRepositoryError::Conflict(
                loaded.snapshot.process_id.clone(),
            ));
        }
        Ok(loaded_after_write(
            replacement_snapshot,
            document,
            recovery_index,
            outbox_index,
        ))
    }
}

impl From<DocumentRepositoryError> for CommitError {
    fn from(error: DocumentRepositoryError) -> Self {
        match error {
            DocumentRepositoryError::EmptyProcessId => CommitError::EmptyProcessId,
            DocumentRepositoryError::KeyEncoding(source) => CommitError::CorruptAggregate(source),
            DocumentRepositoryError::Conflict(process_id) => {
                CommitError::RepositoryConflict(process_id)
            }
            DocumentRepositoryError::ProcessIdentityMismatch { requested, stored } => {
                CommitError::CorruptAggregate(format!(
                    "aggregate key {requested:?} contains process {stored:?}"
                ))
            }
            DocumentRepositoryError::CorruptAggregate { process_id, source } => {
                CommitError::CorruptAggregate(format!("process {process_id:?}: {source}"))
            }
            DocumentRepositoryError::Codec(source) => {
                CommitError::CorruptAggregate(source.to_string())
            }
            DocumentRepositoryError::RecoveryIndex(source) => CommitError::CorruptAggregate(source),
            DocumentRepositoryError::RecoveryIndexMismatch { process_id } => {
                CommitError::CorruptAggregate(format!(
                    "recovery index mismatch for process {process_id:?}"
                ))
            }
            DocumentRepositoryError::OutboxIndex(source) => CommitError::CorruptAggregate(source),
            DocumentRepositoryError::OutboxIndexMismatch { process_id } => {
                CommitError::CorruptAggregate(format!(
                    "outbox index mismatch for process {process_id:?}"
                ))
            }
            DocumentRepositoryError::Store(source) => {
                CommitError::RepositoryUnavailable(source.to_string())
            }
        }
    }
}

impl<S: AtomicDocumentStore> AggregateRepository for AggregateDocumentRepository<S> {
    fn load(&self, process_id: &str) -> Result<Option<AggregateSnapshot>, CommitError> {
        Ok(AggregateDocumentRepository::load(self, process_id)?.map(|loaded| loaded.snapshot))
    }

    fn compare_and_commit(&mut self, plan: &TransitionPlan) -> Result<CommitReceipt, CommitError> {
        let loaded = AggregateDocumentRepository::load(self, &plan.process_id)?
            .ok_or_else(|| CommitError::AggregateNotFound(plan.process_id.clone()))?;
        let applied = apply_transition(&loaded.snapshot, plan)?;
        self.replace(&loaded, &applied.snapshot)?;
        Ok(applied.receipt)
    }
}

impl<S: AtomicDocumentStore> AggregateDocumentRepository<S> {
    fn evolve_operation_document(
        &mut self,
        id: &super::OperationId,
        expected_revision: u64,
        event: super::OperationEvent,
    ) -> Result<super::OperationRecord, CommitError> {
        let loaded = AggregateDocumentRepository::load(self, &id.process_id)?
            .ok_or_else(|| CommitError::AggregateNotFound(id.process_id.clone()))?;
        let applied = apply_operation_event(&loaded.snapshot, id, expected_revision, event)?;
        self.replace(&loaded, &applied.snapshot)?;
        Ok(applied.operation)
    }
}

impl<S: AtomicDocumentStore> OperationRepository for AggregateDocumentRepository<S> {
    fn claim_operation(
        &mut self,
        id: &super::OperationId,
        expected_revision: u64,
        claim: super::ClaimToken,
    ) -> Result<super::OperationRecord, CommitError> {
        self.evolve_operation_document(id, expected_revision, super::OperationEvent::Claim(claim))
    }

    fn renew_operation(
        &mut self,
        id: &super::OperationId,
        expected_revision: u64,
        current: super::ClaimToken,
        renewed: super::ClaimToken,
    ) -> Result<super::OperationRecord, CommitError> {
        self.evolve_operation_document(
            id,
            expected_revision,
            super::OperationEvent::RenewClaim { current, renewed },
        )
    }

    fn prepare_operation(
        &mut self,
        id: &super::OperationId,
        expected_revision: u64,
        claim: super::ClaimToken,
    ) -> Result<super::OperationRecord, CommitError> {
        self.evolve_operation_document(
            id,
            expected_revision,
            super::OperationEvent::PrepareCommit { claim },
        )
    }

    fn commit_operation(
        &mut self,
        id: &super::OperationId,
        expected_revision: u64,
        claim: super::ClaimToken,
        commit: super::CommitToken,
    ) -> Result<super::OperationRecord, CommitError> {
        self.evolve_operation_document(
            id,
            expected_revision,
            super::OperationEvent::Commit { claim, commit },
        )
    }

    fn schedule_operation_retry(
        &mut self,
        id: &super::OperationId,
        expected_revision: u64,
        claim: super::ClaimToken,
        attempt: u32,
    ) -> Result<super::OperationRecord, CommitError> {
        self.evolve_operation_document(
            id,
            expected_revision,
            super::OperationEvent::ScheduleRetry { claim, attempt },
        )
    }

    fn mark_operation_effect_unknown(
        &mut self,
        id: &super::OperationId,
        expected_revision: u64,
        claim: super::ClaimToken,
        attempt: u32,
    ) -> Result<super::OperationRecord, CommitError> {
        self.evolve_operation_document(
            id,
            expected_revision,
            super::OperationEvent::MarkEffectUnknown { claim, attempt },
        )
    }

    fn complete_operation(
        &mut self,
        id: &super::OperationId,
        expected_revision: u64,
        commit: super::CommitToken,
    ) -> Result<super::OperationRecord, CommitError> {
        self.evolve_operation_document(
            id,
            expected_revision,
            super::OperationEvent::Complete { commit },
        )
    }

    fn quarantine_operation(
        &mut self,
        id: &super::OperationId,
        expected_revision: u64,
        claim: super::ClaimToken,
        reason: String,
    ) -> Result<super::OperationRecord, CommitError> {
        self.evolve_operation_document(
            id,
            expected_revision,
            super::OperationEvent::Quarantine { claim, reason },
        )
    }

    fn release_quarantined_operation(
        &mut self,
        id: &super::OperationId,
        expected_revision: u64,
        next_attempt: u32,
    ) -> Result<super::OperationRecord, CommitError> {
        self.evolve_operation_document(
            id,
            expected_revision,
            super::OperationEvent::ReleaseQuarantine { next_attempt },
        )
    }
}

impl<S: AtomicDocumentStore> RecoveryRepository for AggregateDocumentRepository<S> {
    fn recoverable_page(
        &self,
        after: Option<&RecoveryCursor>,
        limit: usize,
    ) -> Result<RecoveryPage, CommitError> {
        if limit == 0 {
            return Err(CommitError::InvalidPageLimit);
        }

        // Recovery scans the compact materialized index, never every aggregate.
        // Aggregate documents are loaded only for the selected page and remain
        // the authoritative source of operation state.
        let mut entries = Vec::new();
        for (key, document) in self
            .store
            .scan_prefix(RECOVERY_INDEX_KEY_PREFIX)
            .map_err(|error| CommitError::RepositoryUnavailable(error.to_string()))?
        {
            let index = RecoveryIndexCodec::decode(&document).map_err(|source| {
                CommitError::CorruptAggregate(format!("recovery index {key:?}: {source}"))
            })?;
            entries.extend(index.entries);
        }
        entries.sort_by(|left, right| operation_order(&left.operation_id, &right.operation_id));
        if let Some(cursor) = after {
            entries.retain(|entry| operation_order(&entry.operation_id, &cursor.0).is_gt());
        }
        let has_more = entries.len() > limit;
        entries.truncate(limit);

        let mut aggregates = HashMap::new();
        let mut operations = Vec::with_capacity(entries.len());
        for entry in entries {
            let process_id = entry.operation_id.process_id.clone();
            if let std::collections::hash_map::Entry::Vacant(slot) =
                aggregates.entry(process_id.clone())
            {
                let loaded =
                    AggregateDocumentRepository::load(self, &process_id)?.ok_or_else(|| {
                        CommitError::CorruptAggregate(format!(
                            "recovery index references missing aggregate {process_id:?}",
                        ))
                    })?;
                slot.insert(loaded.snapshot);
            }
            let snapshot = aggregates.get(&process_id).ok_or_else(|| {
                CommitError::CorruptAggregate(format!(
                    "aggregate cache lost process {process_id:?}",
                ))
            })?;
            let operation = snapshot
                .operations
                .get(&entry.operation_id)
                .ok_or_else(|| {
                    CommitError::CorruptAggregate(format!(
                        "recovery index references missing operation {:?}",
                        entry.operation_id,
                    ))
                })?;
            if operation.revision != entry.revision || operation.is_terminal() {
                return Err(CommitError::CorruptAggregate(format!(
                    "recovery index entry is stale for operation {:?}",
                    entry.operation_id,
                )));
            }
            operations.push(operation.clone());
        }
        let next = if has_more {
            let last = operations.last().ok_or_else(|| {
                CommitError::CorruptAggregate(
                    "recovery index reported more rows for an empty page".into(),
                )
            })?;
            Some(RecoveryCursor(last.id.clone()))
        } else {
            None
        };
        Ok(RecoveryPage { operations, next })
    }

    fn finalize_committed_operation(
        &mut self,
        id: &super::OperationId,
        expected_revision: u64,
        commit: super::CommitToken,
    ) -> Result<super::OperationRecord, CommitError> {
        self.evolve_operation_document(
            id,
            expected_revision,
            super::OperationEvent::Complete { commit },
        )
    }

    fn reclaim_expired_operation(
        &mut self,
        id: &super::OperationId,
        expected_revision: u64,
        now_ms: i64,
    ) -> Result<super::OperationRecord, CommitError> {
        self.evolve_operation_document(
            id,
            expected_revision,
            super::OperationEvent::ReleaseExpiredClaim { now_ms },
        )
    }
}

fn operation_order(left: &super::OperationId, right: &super::OperationId) -> std::cmp::Ordering {
    left.process_id
        .cmp(&right.process_id)
        .then_with(|| left.task_id.cmp(&right.task_id))
        .then_with(|| left.generation.cmp(&right.generation))
        .then_with(|| left.sequence.cmp(&right.sequence))
}

fn execute_transaction<S: AtomicDocumentStore>(
    store: &mut S,
    transaction: &DocumentTransaction,
) -> Result<bool, DocumentStoreError> {
    transaction.validate()?;
    store.atomic_batch(transaction.guards(), transaction.mutations())
}

fn compare_exchange_document<S: AtomicDocumentStore>(
    store: &mut S,
    key: &str,
    expected: Option<&[u8]>,
    replacement: Vec<u8>,
) -> Result<bool, DocumentStoreError> {
    let transaction = match expected {
        Some(expected) => DocumentTransaction::new().require_exact(key, expected.to_vec())?,
        None => DocumentTransaction::new().require_absent(key)?,
    }
    .put(key, replacement)?;
    execute_transaction(store, &transaction)
}

fn validate_atomic_batch(
    guards: &[DocumentGuard],
    mutations: &[DocumentMutation],
) -> Result<(), DocumentStoreError> {
    validate_document_transaction_structure(guards, mutations)?;
    if mutations.is_empty() {
        return Err(DocumentStoreError(
            "document transaction has no mutations".into(),
        ));
    }
    Ok(())
}

fn validate_document_transaction_structure(
    guards: &[DocumentGuard],
    mutations: &[DocumentMutation],
) -> Result<(), DocumentStoreError> {
    let mut guard_keys = std::collections::HashSet::new();
    for guard in guards {
        if guard.key.is_empty() {
            return Err(DocumentStoreError("empty document guard key".into()));
        }
        if !guard_keys.insert(guard.key.as_str()) {
            return Err(DocumentStoreError(format!(
                "duplicate document guard for {:?}",
                guard.key
            )));
        }
    }
    let mut mutation_keys = std::collections::HashSet::new();
    for mutation in mutations {
        let key = match mutation {
            DocumentMutation::Put { key, .. } | DocumentMutation::Delete { key } => key,
        };
        if key.is_empty() {
            return Err(DocumentStoreError("empty document mutation key".into()));
        }
        if !mutation_keys.insert(key.as_str()) {
            return Err(DocumentStoreError(format!(
                "duplicate document mutation for {key:?}"
            )));
        }
    }
    Ok(())
}

pub(super) fn outbox_index_key(process_id: &str) -> Result<String, DocumentRepositoryError> {
    encoded_process_key(OUTBOX_INDEX_KEY_PREFIX, process_id)
}

fn outbox_index_document(
    snapshot: &AggregateSnapshot,
) -> Result<Option<Vec<u8>>, DocumentRepositoryError> {
    OutboxIndex::project(&snapshot.process_id, &snapshot.outbox)
        .map_err(|error| DocumentRepositoryError::OutboxIndex(error.to_string()))?
        .map(|index| {
            OutboxIndexCodec::encode(&index)
                .map_err(|error| DocumentRepositoryError::OutboxIndex(error.to_string()))
        })
        .transpose()
}

pub(super) fn recovery_index_key(process_id: &str) -> Result<String, DocumentRepositoryError> {
    encoded_process_key(RECOVERY_INDEX_KEY_PREFIX, process_id)
}

fn recovery_index_document(
    snapshot: &AggregateSnapshot,
) -> Result<Option<Vec<u8>>, DocumentRepositoryError> {
    RecoveryIndex::project(snapshot)
        .map(|index| {
            RecoveryIndexCodec::encode(&index)
                .map_err(|error| DocumentRepositoryError::RecoveryIndex(error.to_string()))
        })
        .transpose()
}

fn encoded_process_key(prefix: &str, process_id: &str) -> Result<String, DocumentRepositoryError> {
    if process_id.trim().is_empty() {
        return Err(DocumentRepositoryError::EmptyProcessId);
    }
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let encoded_len = process_id
        .len()
        .checked_mul(2)
        .and_then(|length| prefix.len().checked_add(length))
        .ok_or_else(|| {
            DocumentRepositoryError::KeyEncoding("encoded key length overflow".into())
        })?;
    let mut bytes = Vec::with_capacity(encoded_len);
    bytes.extend_from_slice(prefix.as_bytes());
    for byte in process_id.bytes() {
        bytes.push(HEX[usize::from(byte >> 4)]);
        bytes.push(HEX[usize::from(byte & 0x0f)]);
    }
    String::from_utf8(bytes)
        .map_err(|_| DocumentRepositoryError::KeyEncoding("encoded key is not UTF-8".into()))
}

/// The three documents one atomic install writes: the aggregate snapshot
/// bytes plus the expected/replacement recovery-index and outbox-index
/// documents. Shared by the replace and create transaction builders.
pub(super) type TransactionParts = Result<
    (
        DocumentTransaction,
        Vec<u8>,
        Option<Vec<u8>>,
        Option<Vec<u8>>,
    ),
    DocumentRepositoryError,
>;

pub(super) fn build_replace_transaction(
    loaded: &LoadedAggregate,
    replacement_snapshot: &AggregateSnapshot,
) -> TransactionParts {
    if loaded.snapshot.process_id != replacement_snapshot.process_id {
        return Err(DocumentRepositoryError::ProcessIdentityMismatch {
            requested: loaded.snapshot.process_id.clone(),
            stored: replacement_snapshot.process_id.clone(),
        });
    }
    let process_id = &loaded.snapshot.process_id;
    let key = aggregate_key(process_id)?;
    let recovery_key = recovery_index_key(process_id)?;
    let outbox_key = outbox_index_key(process_id)?;
    let document = AggregateDocumentCodec::encode(replacement_snapshot)?;
    let recovery_index = recovery_index_document(replacement_snapshot)?;
    let outbox_index = outbox_index_document(replacement_snapshot)?;
    let mut transaction =
        DocumentTransaction::new().require_exact(&key, loaded.original_document().to_vec())?;
    transaction = match loaded.original_recovery_index() {
        Some(index) => transaction.require_exact(&recovery_key, index.to_vec())?,
        None => transaction.require_absent(&recovery_key)?,
    };
    transaction = match loaded.original_outbox_index() {
        Some(index) => transaction.require_exact(&outbox_key, index.to_vec())?,
        None => transaction.require_absent(&outbox_key)?,
    };
    transaction = transaction.put(&key, document.clone())?;
    transaction = match &recovery_index {
        Some(index) => transaction.put(&recovery_key, index.clone())?,
        None => transaction.delete(&recovery_key)?,
    };
    transaction = match &outbox_index {
        Some(index) => transaction.put(&outbox_key, index.clone())?,
        None => transaction.delete(&outbox_key)?,
    };
    transaction = replace_recovery_entries(transaction, &loaded.snapshot, replacement_snapshot)?;
    transaction.validate()?;
    Ok((transaction, document, recovery_index, outbox_index))
}

fn replace_recovery_entries(
    mut transaction: DocumentTransaction,
    current: &AggregateSnapshot,
    replacement: &AggregateSnapshot,
) -> Result<DocumentTransaction, DocumentStoreError> {
    let current_entries = RecoveryEntryDocument::project(current);
    let replacement_entries = RecoveryEntryDocument::project(replacement);
    for (key, value) in &current_entries {
        transaction = transaction.require_exact(key.as_str(), value.clone())?;
    }
    for key in replacement_entries.keys() {
        if !current_entries.contains_key(key) {
            transaction = transaction.require_absent(key.as_str())?;
        }
    }
    for key in current_entries.keys() {
        if !replacement_entries.contains_key(key) {
            transaction = transaction.delete(key.as_str())?;
        }
    }
    for (key, value) in replacement_entries {
        transaction = transaction.put(key, value)?;
    }
    Ok(transaction)
}

pub(super) fn build_create_transaction(snapshot: &AggregateSnapshot) -> TransactionParts {
    let key = aggregate_key(&snapshot.process_id)?;
    let recovery_key = recovery_index_key(&snapshot.process_id)?;
    let outbox_key = outbox_index_key(&snapshot.process_id)?;
    let document = AggregateDocumentCodec::encode(snapshot)?;
    let recovery_index = recovery_index_document(snapshot)?;
    let outbox_index = outbox_index_document(snapshot)?;
    let mut transaction = DocumentTransaction::new()
        .require_absent(&key)?
        .require_absent(&recovery_key)?
        .require_absent(&outbox_key)?
        .put(&key, document.clone())?;
    transaction = match &recovery_index {
        Some(index) => transaction.put(&recovery_key, index.clone())?,
        None => transaction.delete(&recovery_key)?,
    };
    transaction = match &outbox_index {
        Some(index) => transaction.put(&outbox_key, index.clone())?,
        None => transaction.delete(&outbox_key)?,
    };
    for (entry_key, entry_value) in RecoveryEntryDocument::project(snapshot) {
        transaction = transaction
            .require_absent(entry_key.as_str())?
            .put(entry_key, entry_value)?;
    }
    transaction.validate()?;
    Ok((transaction, document, recovery_index, outbox_index))
}

pub(super) fn loaded_after_write(
    snapshot: &AggregateSnapshot,
    document: Vec<u8>,
    recovery_index: Option<Vec<u8>>,
    outbox_index: Option<Vec<u8>>,
) -> LoadedAggregate {
    LoadedAggregate {
        snapshot: snapshot.clone(),
        original_document: document,
        original_recovery_index: recovery_index,
        original_outbox_index: outbox_index,
    }
}

pub(super) fn decode_loaded_aggregate(
    process_id: &str,
    original_document: Vec<u8>,
    original_recovery_index: Option<Vec<u8>>,
    original_outbox_index: Option<Vec<u8>>,
) -> Result<LoadedAggregate, DocumentRepositoryError> {
    let snapshot = AggregateDocumentCodec::decode(&original_document).map_err(|source| {
        DocumentRepositoryError::CorruptAggregate {
            process_id: process_id.into(),
            source,
        }
    })?;
    if snapshot.process_id != process_id {
        return Err(DocumentRepositoryError::ProcessIdentityMismatch {
            requested: process_id.into(),
            stored: snapshot.process_id,
        });
    }
    let expected_recovery_index = recovery_index_document(&snapshot)?;
    if original_recovery_index.as_deref() != expected_recovery_index.as_deref() {
        return Err(DocumentRepositoryError::RecoveryIndexMismatch {
            process_id: process_id.into(),
        });
    }
    let expected_outbox_index = outbox_index_document(&snapshot)?;
    if original_outbox_index.as_deref() != expected_outbox_index.as_deref() {
        return Err(DocumentRepositoryError::OutboxIndexMismatch {
            process_id: process_id.into(),
        });
    }
    Ok(LoadedAggregate {
        snapshot,
        original_document,
        original_recovery_index,
        original_outbox_index,
    })
}

pub(super) fn aggregate_key(process_id: &str) -> Result<String, DocumentRepositoryError> {
    encoded_process_key(AGGREGATE_KEY_PREFIX, process_id)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DocumentRepositoryError {
    EmptyProcessId,
    KeyEncoding(String),
    Conflict(String),
    ProcessIdentityMismatch {
        requested: String,
        stored: String,
    },
    CorruptAggregate {
        process_id: String,
        source: AggregateDocumentError,
    },
    Codec(AggregateDocumentError),
    RecoveryIndex(String),
    RecoveryIndexMismatch {
        process_id: String,
    },
    OutboxIndex(String),
    OutboxIndexMismatch {
        process_id: String,
    },
    Store(DocumentStoreError),
}
impl From<AggregateDocumentError> for DocumentRepositoryError {
    fn from(error: AggregateDocumentError) -> Self {
        Self::Codec(error)
    }
}
impl From<DocumentStoreError> for DocumentRepositoryError {
    fn from(error: DocumentStoreError) -> Self {
        Self::Store(error)
    }
}
impl fmt::Display for DocumentRepositoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "document repository error: {self:?}")
    }
}
impl std::error::Error for DocumentRepositoryError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DocumentStoreError(pub(crate) String);
impl fmt::Display for DocumentStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for DocumentStoreError {}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct MemoryDocumentStore {
    documents: HashMap<String, Vec<u8>>,
}
impl AtomicDocumentStore for MemoryDocumentStore {
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, DocumentStoreError> {
        Ok(self.documents.get(key).cloned())
    }

    fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, DocumentStoreError> {
        let mut documents = self
            .documents
            .iter()
            .filter(|(key, _)| key.starts_with(prefix))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Vec<_>>();
        documents.sort_by(|(left, _), (right, _)| left.cmp(right));
        Ok(documents)
    }

    fn atomic_batch(
        &mut self,
        guards: &[DocumentGuard],
        mutations: &[DocumentMutation],
    ) -> Result<bool, DocumentStoreError> {
        validate_atomic_batch(guards, mutations)?;
        let all_match = guards.iter().all(|guard| {
            let current = self.documents.get(&guard.key).map(Vec::as_slice);
            match (&guard.expected, current) {
                (None, None) => true,
                (Some(expected), Some(current)) => expected.as_slice() == current,
                _ => false,
            }
        });
        if !all_match {
            return Ok(false);
        }
        for mutation in mutations {
            match mutation {
                DocumentMutation::Put { key, value } => {
                    self.documents.insert(key.clone(), value.clone());
                }
                DocumentMutation::Delete { key } => {
                    self.documents.remove(key);
                }
            }
        }
        Ok(true)
    }
}

impl<S: AtomicDocumentStore> OutboxRepository for AggregateDocumentRepository<S> {
    fn load_outbox(&self, id: &OutboxId) -> Result<Option<OutboxRecord>, OutboxRepositoryError> {
        let loaded = AggregateDocumentRepository::load(self, &id.process_id)
            .map_err(|error| OutboxRepositoryError::Unavailable(error.to_string()))?;
        Ok(loaded.and_then(|aggregate| aggregate.snapshot.outbox.get(id).cloned()))
    }

    fn dispatchable_outbox_page(
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
        let mut aggregates = HashMap::new();
        let mut records = Vec::with_capacity(entries.len());
        for entry in entries {
            let process_id = entry.id.process_id.clone();
            if !aggregates.contains_key(&process_id) {
                let loaded = AggregateDocumentRepository::load(self, &process_id)
                    .map_err(|error| OutboxRepositoryError::Unavailable(error.to_string()))?
                    .ok_or_else(|| {
                        OutboxRepositoryError::CorruptData(format!(
                            "outbox index references missing aggregate {process_id:?}"
                        ))
                    })?;
                aggregates.insert(process_id.clone(), loaded.snapshot);
            }
            let record = aggregates[&process_id]
                .outbox
                .get(&entry.id)
                .ok_or_else(|| {
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
            let last = records.last().ok_or_else(|| {
                OutboxRepositoryError::CorruptData(
                    "outbox index reported more rows for an empty page".into(),
                )
            })?;
            Some(OutboxCursor(last.id.clone()))
        } else {
            None
        };
        Ok(OutboxPage { records, next })
    }

    fn evolve_outbox(
        &mut self,
        id: &OutboxId,
        expected_revision: u64,
        event: OutboxEvent,
    ) -> Result<OutboxRecord, OutboxRepositoryError> {
        let loaded = AggregateDocumentRepository::load(self, &id.process_id)
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
        self.replace(&loaded, &replacement)
            .map_err(|error| match error {
                DocumentRepositoryError::Conflict(_) => {
                    OutboxRepositoryError::ConcurrentModification(id.clone())
                }
                other => OutboxRepositoryError::Unavailable(other.to_string()),
            })?;
        Ok(evolved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::{AggregateVersion, ExecutionId, ExecutionSnapshot, ExecutionState, NodeId};
    use std::collections::{BTreeMap, HashMap};

    fn snapshot(process_id: &str, revision: u64) -> AggregateSnapshot {
        let execution = ExecutionId::new("root", 0).unwrap();
        AggregateSnapshot {
            process_id: process_id.into(),
            version: AggregateVersion {
                graph: 0,
                process: revision,
                operations: 0,
            },
            executions: HashMap::from([(
                execution,
                ExecutionSnapshot {
                    model_node: NodeId::parse("root").unwrap(),
                    parent: None,
                    predecessors: vec![],
                    state: ExecutionState::Waiting,
                    revision,
                },
            )]),
            variables: BTreeMap::new(),
            operations: HashMap::new(),
            outbox: HashMap::new(),
        }
    }

    #[test]
    fn aggregate_key_is_unambiguous_for_separator_characters() {
        assert_ne!(
            aggregate_key("a/b").unwrap(),
            aggregate_key("a:2fb").unwrap()
        );
        assert_eq!(aggregate_key("é").unwrap(), "kernel/aggregates/c3a9");
    }

    #[test]
    fn create_requires_absence_and_never_overwrites() {
        let mut repository = AggregateDocumentRepository::new(MemoryDocumentStore::default());
        repository.create(&snapshot("p", 0)).unwrap();
        assert_eq!(
            repository.create(&snapshot("p", 1)),
            Err(DocumentRepositoryError::Conflict("p".into()))
        );
        assert_eq!(
            repository
                .load("p")
                .unwrap()
                .unwrap()
                .snapshot
                .version
                .process,
            0
        );
    }

    #[test]
    fn concurrent_writers_use_exact_original_bytes_and_one_loses() {
        let mut repository = AggregateDocumentRepository::new(MemoryDocumentStore::default());
        repository.create(&snapshot("p", 0)).unwrap();
        let writer_a = repository.load("p").unwrap().unwrap();
        let writer_b = repository.load("p").unwrap().unwrap();
        repository.replace(&writer_a, &snapshot("p", 1)).unwrap();
        assert_eq!(
            repository.replace(&writer_b, &snapshot("p", 2)),
            Err(DocumentRepositoryError::Conflict("p".into()))
        );
        assert_eq!(
            repository
                .load("p")
                .unwrap()
                .unwrap()
                .snapshot
                .version
                .process,
            1
        );
    }

    #[test]
    fn conflict_is_zero_write_and_loaded_token_is_opaque_original_bytes() {
        let mut repository = AggregateDocumentRepository::new(MemoryDocumentStore::default());
        let loaded = repository.create(&snapshot("p", 0)).unwrap();
        let original = loaded.original_document().to_vec();
        let committed = repository.replace(&loaded, &snapshot("p", 1)).unwrap();
        assert_ne!(committed.original_document(), original);
        assert_eq!(
            repository.replace(&loaded, &snapshot("p", 9)),
            Err(DocumentRepositoryError::Conflict("p".into()))
        );
        assert_eq!(
            repository
                .load("p")
                .unwrap()
                .unwrap()
                .snapshot
                .version
                .process,
            1
        );
    }

    #[test]
    fn corrupt_document_is_not_reported_as_missing_or_conflict() {
        let key = aggregate_key("p").unwrap();
        let mut store = MemoryDocumentStore::default();
        store.documents.insert(key, b"corrupt".to_vec());
        let repository = AggregateDocumentRepository::new(store);
        assert!(matches!(
            repository.load("p"),
            Err(DocumentRepositoryError::CorruptAggregate { .. })
        ));
    }

    #[test]
    fn requested_and_stored_process_identity_must_match() {
        let mut store = MemoryDocumentStore::default();
        store.documents.insert(
            aggregate_key("requested").unwrap(),
            AggregateDocumentCodec::encode(&snapshot("stored", 0)).unwrap(),
        );
        let repository = AggregateDocumentRepository::new(store);
        assert_eq!(
            repository.load("requested"),
            Err(DocumentRepositoryError::ProcessIdentityMismatch {
                requested: "requested".into(),
                stored: "stored".into()
            })
        );
    }
    #[derive(Default)]
    struct FailingCompareExchangeStore {
        inner: MemoryDocumentStore,
        fail_next: bool,
    }
    impl AtomicDocumentStore for FailingCompareExchangeStore {
        fn read(&self, key: &str) -> Result<Option<Vec<u8>>, DocumentStoreError> {
            self.inner.read(key)
        }
        fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, DocumentStoreError> {
            self.inner.scan_prefix(prefix)
        }
        fn atomic_batch(
            &mut self,
            guards: &[DocumentGuard],
            mutations: &[DocumentMutation],
        ) -> Result<bool, DocumentStoreError> {
            if self.fail_next {
                self.fail_next = false;
                return Err(DocumentStoreError("injected atomic write failure".into()));
            }
            self.inner.atomic_batch(guards, mutations)
        }
    }

    #[test]
    fn storage_failure_never_installs_replacement_document() {
        let mut store = FailingCompareExchangeStore::default();
        compare_exchange_document(
            &mut store.inner,
            &aggregate_key("p").unwrap(),
            None,
            AggregateDocumentCodec::encode(&snapshot("p", 0)).unwrap(),
        )
        .unwrap();
        store.fail_next = true;
        let mut repository = AggregateDocumentRepository::new(store);
        let loaded = repository.load("p").unwrap().unwrap();
        assert_eq!(
            repository.replace(&loaded, &snapshot("p", 1)),
            Err(DocumentRepositoryError::Store(DocumentStoreError(
                "injected atomic write failure".into()
            ))),
        );
        assert_eq!(
            repository
                .load("p")
                .unwrap()
                .unwrap()
                .snapshot
                .version
                .process,
            0
        );
    }

    #[test]
    fn invalid_replacement_is_rejected_before_store_mutation() {
        let mut repository = AggregateDocumentRepository::new(MemoryDocumentStore::default());
        let loaded = repository.create(&snapshot("p", 0)).unwrap();
        let mut invalid = snapshot("p", 1);
        invalid.process_id = " ".into();
        assert!(matches!(
            repository.replace(&loaded, &invalid),
            Err(DocumentRepositoryError::ProcessIdentityMismatch { .. })
        ));
        assert_eq!(
            repository
                .load("p")
                .unwrap()
                .unwrap()
                .snapshot
                .version
                .process,
            0
        );
    }

    fn transition_plan(process_id: &str, expected_revision: u64) -> TransitionPlan {
        use crate::kernel::{StateChange, VariableWrite};
        TransitionPlan {
            process_id: process_id.into(),
            expected: AggregateVersion {
                graph: 0,
                process: expected_revision,
                operations: 0,
            },
            execution_creates: vec![],
            state_changes: vec![StateChange {
                execution: ExecutionId::new("root", 0).unwrap(),
                expected_revision,
                from: ExecutionState::Waiting,
                to: ExecutionState::Running,
            }],
            variable_writes: vec![VariableWrite {
                scope: ExecutionId::new("root", 0).unwrap(),
                name: "result".into(),
                value: "42".into(),
            }],
            source_operation: None,
            operations: vec![],
            outbox: vec![],
        }
    }

    #[test]
    fn document_repository_implements_the_same_aggregate_commit_contract() {
        let mut repository = AggregateDocumentRepository::new(MemoryDocumentStore::default());
        repository.create(&snapshot("p", 0)).unwrap();
        let receipt =
            AggregateRepository::compare_and_commit(&mut repository, &transition_plan("p", 0))
                .unwrap();
        let committed = AggregateRepository::load(&repository, "p")
            .unwrap()
            .unwrap();
        assert_eq!(receipt.current, committed.version);
        assert_eq!(
            committed.executions[&ExecutionId::new("root", 0).unwrap()].state,
            ExecutionState::Running
        );
        assert_eq!(
            committed.variables[&(ExecutionId::new("root", 0).unwrap(), "result".into())],
            "42"
        );
    }

    #[test]
    fn stale_document_commit_is_rejected_without_installing_precomputed_state() {
        let mut repository = AggregateDocumentRepository::new(MemoryDocumentStore::default());
        repository.create(&snapshot("p", 0)).unwrap();
        AggregateRepository::compare_and_commit(&mut repository, &transition_plan("p", 0)).unwrap();
        let stale = transition_plan("p", 0);
        assert!(matches!(
            AggregateRepository::compare_and_commit(&mut repository, &stale),
            Err(CommitError::VersionConflict { .. })
        ));
        let committed = AggregateRepository::load(&repository, "p")
            .unwrap()
            .unwrap();
        assert_eq!(committed.version.process, 1);
    }

    #[test]
    fn document_operation_repository_persists_claim_and_revision_together() {
        use crate::kernel::{
            ClaimToken, OperationId, OperationKind, OperationRecord, OperationState,
        };
        let operation_id = OperationId {
            process_id: "p".into(),
            task_id: "root".into(),
            generation: 0,
            sequence: 1,
        };
        let mut source = snapshot("p", 0);
        source.operations.insert(
            operation_id.clone(),
            OperationRecord::pending(operation_id.clone(), OperationKind::ExecuteTask),
        );
        let mut repository = AggregateDocumentRepository::new(MemoryDocumentStore::default());
        repository.create(&source).unwrap();
        let claim = ClaimToken {
            owner: "runtime-a".into(),
            fence: 3,
            lease_until_ms: 100,
            heartbeat_revision: 0,
        };
        let claimed =
            OperationRepository::claim_operation(&mut repository, &operation_id, 0, claim.clone())
                .unwrap();
        assert_eq!(claimed.state, OperationState::Claimed(claim));
        assert_eq!(claimed.revision, 1);
        let stored = AggregateRepository::load(&repository, "p")
            .unwrap()
            .unwrap();
        assert_eq!(stored.operations[&operation_id], claimed);
        assert_eq!(stored.version.operations, 1);
    }

    #[test]
    fn stale_operation_writer_cannot_overwrite_new_claim() {
        use crate::kernel::{ClaimToken, OperationId, OperationKind, OperationRecord};
        let operation_id = OperationId {
            process_id: "p".into(),
            task_id: "root".into(),
            generation: 0,
            sequence: 1,
        };
        let mut source = snapshot("p", 0);
        source.operations.insert(
            operation_id.clone(),
            OperationRecord::pending(operation_id.clone(), OperationKind::ExecuteTask),
        );
        let mut repository = AggregateDocumentRepository::new(MemoryDocumentStore::default());
        repository.create(&source).unwrap();
        let first = ClaimToken {
            owner: "runtime-a".into(),
            fence: 1,
            lease_until_ms: 100,
            heartbeat_revision: 0,
        };
        OperationRepository::claim_operation(&mut repository, &operation_id, 0, first).unwrap();
        let stale = ClaimToken {
            owner: "runtime-b".into(),
            fence: 2,
            lease_until_ms: 200,
            heartbeat_revision: 0,
        };
        assert!(matches!(
            OperationRepository::claim_operation(&mut repository, &operation_id, 0, stale),
            Err(CommitError::OperationRevisionConflict { .. })
        ));
        assert_eq!(
            AggregateRepository::load(&repository, "p")
                .unwrap()
                .unwrap()
                .operations[&operation_id]
                .revision,
            1
        );
    }

    #[test]
    fn document_recovery_finalizes_committed_operation_through_cas() {
        use crate::kernel::{
            ClaimToken, CommitToken, OperationEvent, OperationId, OperationKind, OperationRecord,
            OperationState,
        };
        let id = OperationId {
            process_id: "p".into(),
            task_id: "root".into(),
            generation: 0,
            sequence: 8,
        };
        let claim = ClaimToken {
            owner: "runtime".into(),
            fence: 1,
            lease_until_ms: 100,
            heartbeat_revision: 0,
        };
        let commit = CommitToken {
            process_version: 1,
            digest: "a".repeat(64),
        };
        let record = OperationRecord::pending(id.clone(), OperationKind::ExecuteTask)
            .evolve(OperationEvent::Claim(claim.clone()))
            .unwrap()
            .evolve(OperationEvent::PrepareCommit {
                claim: claim.clone(),
            })
            .unwrap()
            .evolve(OperationEvent::Commit {
                claim,
                commit: commit.clone(),
            })
            .unwrap();
        let revision = record.revision;
        let mut source = snapshot("p", 0);
        source.operations.insert(id.clone(), record);
        let mut repository = AggregateDocumentRepository::new(MemoryDocumentStore::default());
        repository.create(&source).unwrap();
        let completed = RecoveryRepository::finalize_committed_operation(
            &mut repository,
            &id,
            revision,
            commit.clone(),
        )
        .unwrap();
        assert_eq!(completed.state, OperationState::Completed(commit));
        assert_eq!(
            AggregateRepository::load(&repository, "p")
                .unwrap()
                .unwrap()
                .operations[&id],
            completed
        );
    }

    #[test]
    fn document_recovery_pages_are_global_stable_and_skip_terminal_operations() {
        use crate::kernel::{OperationId, OperationKind, OperationRecord};
        let mut repository = AggregateDocumentRepository::new(MemoryDocumentStore::default());
        for process in ["b", "a"] {
            let mut source = snapshot(process, 0);
            for sequence in 0..3 {
                let id = OperationId {
                    process_id: process.into(),
                    task_id: "root".into(),
                    generation: 0,
                    sequence,
                };
                source.operations.insert(
                    id.clone(),
                    OperationRecord::pending(id, OperationKind::ExecuteTask),
                );
            }
            repository.create(&source).unwrap();
        }
        let first = RecoveryRepository::recoverable_page(&repository, None, 4).unwrap();
        let second =
            RecoveryRepository::recoverable_page(&repository, first.next.as_ref(), 4).unwrap();
        assert_eq!(
            first
                .operations
                .iter()
                .map(|operation| (operation.id.process_id.as_str(), operation.id.sequence))
                .collect::<Vec<_>>(),
            [("a", 0), ("a", 1), ("a", 2), ("b", 0)],
        );
        assert_eq!(
            second
                .operations
                .iter()
                .map(|operation| operation.id.sequence)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        assert!(second.next.is_none());
    }

    #[test]
    fn atomic_batch_applies_all_mutations_or_none() {
        let mut store = MemoryDocumentStore::default();
        assert!(
            store
                .atomic_batch(
                    &[
                        DocumentGuard {
                            key: "a".into(),
                            expected: None
                        },
                        DocumentGuard {
                            key: "b".into(),
                            expected: None
                        }
                    ],
                    &[
                        DocumentMutation::Put {
                            key: "a".into(),
                            value: b"one".to_vec()
                        },
                        DocumentMutation::Put {
                            key: "b".into(),
                            value: b"two".to_vec()
                        }
                    ],
                )
                .unwrap()
        );
        assert_eq!(store.read("a").unwrap(), Some(b"one".to_vec()));
        assert_eq!(store.read("b").unwrap(), Some(b"two".to_vec()));

        assert!(
            !store
                .atomic_batch(
                    &[DocumentGuard {
                        key: "a".into(),
                        expected: Some(b"stale".to_vec())
                    }],
                    &[
                        DocumentMutation::Put {
                            key: "a".into(),
                            value: b"changed".to_vec()
                        },
                        DocumentMutation::Delete { key: "b".into() }
                    ],
                )
                .unwrap()
        );
        assert_eq!(store.read("a").unwrap(), Some(b"one".to_vec()));
        assert_eq!(store.read("b").unwrap(), Some(b"two".to_vec()));
    }

    #[test]
    fn malformed_atomic_batch_is_rejected_before_any_mutation() {
        let mut store = MemoryDocumentStore::default();
        assert!(
            store
                .atomic_batch(
                    &[],
                    &[DocumentMutation::Put {
                        key: "a".into(),
                        value: b"old".to_vec()
                    }]
                )
                .unwrap()
        );
        let error = store
            .atomic_batch(
                &[],
                &[
                    DocumentMutation::Put {
                        key: "a".into(),
                        value: b"new".to_vec(),
                    },
                    DocumentMutation::Delete { key: "a".into() },
                ],
            )
            .unwrap_err();
        assert!(error.0.contains("duplicate document mutation"));
        assert_eq!(store.read("a").unwrap(), Some(b"old".to_vec()));
    }

    #[test]
    fn document_transaction_rejects_ambiguity_during_construction() {
        let duplicate_guard = DocumentTransaction::new()
            .require_absent("aggregate")
            .unwrap()
            .require_exact("aggregate", b"old".to_vec());
        assert!(
            matches!(duplicate_guard, Err(DocumentStoreError(message)) if message.contains("duplicate document guard"))
        );

        let duplicate_mutation = DocumentTransaction::new()
            .put("index", b"value".to_vec())
            .unwrap()
            .delete("index");
        assert!(
            matches!(duplicate_mutation, Err(DocumentStoreError(message)) if message.contains("duplicate document mutation"))
        );
    }

    #[test]
    fn transaction_can_install_aggregate_and_indexes_in_one_commit() {
        let mut store = MemoryDocumentStore::default();
        let transaction = DocumentTransaction::new()
            .require_absent("aggregate")
            .unwrap()
            .require_absent("recovery-index")
            .unwrap()
            .require_absent("outbox-index")
            .unwrap()
            .put("aggregate", b"aggregate-v1".to_vec())
            .unwrap()
            .put("recovery-index", b"recoverable".to_vec())
            .unwrap()
            .put("outbox-index", b"pending".to_vec())
            .unwrap();
        assert!(execute_transaction(&mut store, &transaction).unwrap());
        assert_eq!(
            store.read("aggregate").unwrap(),
            Some(b"aggregate-v1".to_vec())
        );
        assert_eq!(
            store.read("recovery-index").unwrap(),
            Some(b"recoverable".to_vec())
        );
        assert_eq!(
            store.read("outbox-index").unwrap(),
            Some(b"pending".to_vec())
        );
    }

    #[test]
    fn transaction_guard_conflict_keeps_aggregate_and_indexes_consistent() {
        let mut store = MemoryDocumentStore::default();
        let initial = DocumentTransaction::new()
            .put("aggregate", b"v1".to_vec())
            .unwrap()
            .put("recovery-index", b"old".to_vec())
            .unwrap();
        assert!(execute_transaction(&mut store, &initial).unwrap());

        let stale = DocumentTransaction::new()
            .require_exact("aggregate", b"stale".to_vec())
            .unwrap()
            .require_exact("recovery-index", b"old".to_vec())
            .unwrap()
            .put("aggregate", b"v2".to_vec())
            .unwrap()
            .put("recovery-index", b"new".to_vec())
            .unwrap();
        assert!(!execute_transaction(&mut store, &stale).unwrap());
        assert_eq!(store.read("aggregate").unwrap(), Some(b"v1".to_vec()));
        assert_eq!(store.read("recovery-index").unwrap(), Some(b"old".to_vec()));
    }

    #[test]
    fn operation_transition_updates_aggregate_and_recovery_index_atomically() {
        use crate::kernel::{OperationId, OperationKind, OperationRecord};
        let id = OperationId {
            process_id: "p".into(),
            task_id: "root".into(),
            generation: 0,
            sequence: 1,
        };
        let mut source = snapshot("p", 0);
        source.operations.insert(
            id.clone(),
            OperationRecord::pending(id.clone(), OperationKind::ExecuteTask),
        );
        let mut repository = AggregateDocumentRepository::new(MemoryDocumentStore::default());
        let loaded = repository.create(&source).unwrap();
        assert_eq!(
            loaded.original_recovery_index(),
            recovery_index_document(&source).unwrap().as_deref()
        );
        let claim = crate::kernel::ClaimToken {
            owner: "operator".into(),
            fence: 1,
            lease_until_ms: i64::MAX,
            heartbeat_revision: 0,
        };
        let terminal = source.operations[&id]
            .clone()
            .evolve(crate::kernel::OperationEvent::Claim(claim.clone()))
            .unwrap()
            .evolve(crate::kernel::OperationEvent::Quarantine {
                claim,
                reason: "manual".into(),
            })
            .unwrap();
        let mut next = source.clone();
        next.operations.insert(id, terminal);
        next.version.operations += 1;
        let committed = repository.replace(&loaded, &next).unwrap();
        assert!(committed.original_recovery_index().is_none());
        assert!(
            repository
                .store
                .read(&recovery_index_key("p").unwrap())
                .unwrap()
                .is_none()
        );
    }

    #[derive(Default)]
    struct ScanRecordingStore {
        inner: MemoryDocumentStore,
        scanned_prefixes: std::cell::RefCell<Vec<String>>,
    }
    impl AtomicDocumentStore for ScanRecordingStore {
        fn read(&self, key: &str) -> Result<Option<Vec<u8>>, DocumentStoreError> {
            self.inner.read(key)
        }
        fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, DocumentStoreError> {
            self.scanned_prefixes.borrow_mut().push(prefix.into());
            self.inner.scan_prefix(prefix)
        }
        fn atomic_batch(
            &mut self,
            guards: &[DocumentGuard],
            mutations: &[DocumentMutation],
        ) -> Result<bool, DocumentStoreError> {
            self.inner.atomic_batch(guards, mutations)
        }
    }

    #[test]
    fn recovery_scans_only_indexes_and_loads_only_page_aggregates() {
        use crate::kernel::{OperationId, OperationKind, OperationRecord};
        let mut repository = AggregateDocumentRepository::new(ScanRecordingStore::default());
        for process in ["a", "b", "c"] {
            let mut source = snapshot(process, 0);
            let id = OperationId {
                process_id: process.into(),
                task_id: "root".into(),
                generation: 0,
                sequence: 0,
            };
            source.operations.insert(
                id.clone(),
                OperationRecord::pending(id, OperationKind::ExecuteTask),
            );
            repository.create(&source).unwrap();
        }
        let page = RecoveryRepository::recoverable_page(&repository, None, 1).unwrap();
        assert_eq!(page.operations.len(), 1);
        assert_eq!(
            repository.store.scanned_prefixes.borrow().as_slice(),
            [RECOVERY_INDEX_KEY_PREFIX]
        );
    }

    fn snapshot_with_outbox(process_id: &str) -> (AggregateSnapshot, OutboxId) {
        let mut source = snapshot(process_id, 0);
        let id = OutboxId {
            process_id: process_id.into(),
            key: "event:1".into(),
        };
        let record =
            OutboxRecord::pending(id.clone(), "task.completed".into(), "{}".into(), 0).unwrap();
        source.outbox.insert(id.clone(), record);
        (source, id)
    }

    #[test]
    fn outbox_evolution_atomically_replaces_authoritative_aggregate_and_index() {
        let (source, id) = snapshot_with_outbox("p");
        let mut repository = AggregateDocumentRepository::new(MemoryDocumentStore::default());
        repository.create(&source).unwrap();
        let claim = crate::kernel::DeliveryClaim {
            owner: "worker".into(),
            fence: 1,
            lease_until_ms: 100,
            heartbeat_revision: 0,
        };
        let claimed = OutboxRepository::evolve_outbox(
            &mut repository,
            &id,
            0,
            OutboxEvent::Claim {
                claim: claim.clone(),
                now_ms: 0,
            },
        )
        .unwrap();
        assert_eq!(claimed.revision, 1);
        let loaded = AggregateDocumentRepository::load(&repository, "p")
            .unwrap()
            .unwrap();
        assert_eq!(loaded.snapshot.outbox.get(&id), Some(&claimed));
        let index = OutboxIndexCodec::decode(loaded.original_outbox_index().unwrap()).unwrap();
        assert_eq!(index.entries.len(), 1);
        assert_eq!(index.entries[0].revision, 1);
        assert!(matches!(
            index.entries[0].state,
            OutboxIndexState::Claimed {
                lease_until_ms: 100
            }
        ));
    }

    #[test]
    fn outbox_index_tampering_blocks_authoritative_aggregate_load() {
        let (source, _) = snapshot_with_outbox("p");
        let mut repository = AggregateDocumentRepository::new(MemoryDocumentStore::default());
        repository.create(&source).unwrap();
        let key = outbox_index_key("p").unwrap();
        repository.store.documents.insert(key, vec![0, 1, 2]);
        assert!(matches!(
            AggregateDocumentRepository::load(&repository, "p"),
            Err(DocumentRepositoryError::OutboxIndexMismatch { .. })
        ));
    }

    #[test]
    fn dispatchable_page_uses_index_but_returns_authoritative_records() {
        let (source, id) = snapshot_with_outbox("p");
        let mut repository = AggregateDocumentRepository::new(MemoryDocumentStore::default());
        repository.create(&source).unwrap();
        let page = OutboxRepository::dispatchable_outbox_page(&repository, 0, None, 10).unwrap();
        assert_eq!(page.records.len(), 1);
        assert_eq!(page.records[0].id, id);
        assert_eq!(page.records[0], source.outbox[&id]);
        assert!(page.next.is_none());
    }

    #[test]
    fn delivered_outbox_is_atomically_removed_from_index_but_retained_in_aggregate() {
        let (source, id) = snapshot_with_outbox("p");
        let mut repository = AggregateDocumentRepository::new(MemoryDocumentStore::default());
        repository.create(&source).unwrap();
        let claim = crate::kernel::DeliveryClaim {
            owner: "worker".into(),
            fence: 1,
            lease_until_ms: 100,
            heartbeat_revision: 0,
        };
        let claimed = OutboxRepository::evolve_outbox(
            &mut repository,
            &id,
            0,
            OutboxEvent::Claim {
                claim: claim.clone(),
                now_ms: 0,
            },
        )
        .unwrap();
        let delivered = OutboxRepository::evolve_outbox(
            &mut repository,
            &id,
            claimed.revision,
            OutboxEvent::ConfirmDelivered {
                claim,
                receipt: "receipt-1".into(),
                now_ms: 0,
            },
        )
        .unwrap();
        let loaded = AggregateDocumentRepository::load(&repository, "p")
            .unwrap()
            .unwrap();
        assert_eq!(loaded.snapshot.outbox.get(&id), Some(&delivered));
        assert!(loaded.original_outbox_index().is_none());
        assert!(
            repository
                .store
                .read(&outbox_index_key("p").unwrap())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn failed_atomic_outbox_commit_changes_neither_aggregate_nor_index() {
        let (source, id) = snapshot_with_outbox("p");
        let mut initial = AggregateDocumentRepository::new(MemoryDocumentStore::default());
        initial.create(&source).unwrap();
        let mut store = FailingCompareExchangeStore {
            inner: initial.store,
            fail_next: true,
        };
        let original_aggregate = store.inner.read(&aggregate_key("p").unwrap()).unwrap();
        let original_index = store.inner.read(&outbox_index_key("p").unwrap()).unwrap();
        let claim = crate::kernel::DeliveryClaim {
            owner: "worker".into(),
            fence: 1,
            lease_until_ms: 100,
            heartbeat_revision: 0,
        };
        let mut repository = AggregateDocumentRepository::new(store);
        let result = OutboxRepository::evolve_outbox(
            &mut repository,
            &id,
            0,
            OutboxEvent::Claim { claim, now_ms: 0 },
        );
        assert!(matches!(result, Err(OutboxRepositoryError::Unavailable(_))));
        store = repository.store;
        assert_eq!(
            store.inner.read(&aggregate_key("p").unwrap()).unwrap(),
            original_aggregate
        );
        assert_eq!(
            store.inner.read(&outbox_index_key("p").unwrap()).unwrap(),
            original_index
        );
    }

    #[test]
    fn stale_loaded_aggregate_cannot_overwrite_concurrent_outbox_change() {
        let (source, id) = snapshot_with_outbox("p");
        let mut repository = AggregateDocumentRepository::new(MemoryDocumentStore::default());
        let stale = repository.create(&source).unwrap();
        let claim = crate::kernel::DeliveryClaim {
            owner: "worker".into(),
            fence: 1,
            lease_until_ms: 100,
            heartbeat_revision: 0,
        };
        OutboxRepository::evolve_outbox(
            &mut repository,
            &id,
            0,
            OutboxEvent::Claim { claim, now_ms: 0 },
        )
        .unwrap();
        let mut stale_replacement = stale.snapshot.clone();
        stale_replacement.version.process += 1;
        assert!(matches!(
            repository.replace(&stale, &stale_replacement),
            Err(DocumentRepositoryError::Conflict(_))
        ));
        let authoritative = AggregateDocumentRepository::load(&repository, "p")
            .unwrap()
            .unwrap();
        assert_eq!(authoritative.snapshot.outbox[&id].revision, 1);
    }
    struct ApplyThenLoseResponseStore {
        inner: MemoryDocumentStore,
        lose_next_success_response: bool,
    }

    impl AtomicDocumentStore for ApplyThenLoseResponseStore {
        fn read(&self, key: &str) -> Result<Option<Vec<u8>>, DocumentStoreError> {
            self.inner.read(key)
        }

        fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, DocumentStoreError> {
            self.inner.scan_prefix(prefix)
        }

        fn atomic_batch(
            &mut self,
            guards: &[DocumentGuard],
            mutations: &[DocumentMutation],
        ) -> Result<bool, DocumentStoreError> {
            let committed = self.inner.atomic_batch(guards, mutations)?;
            if committed && self.lose_next_success_response {
                self.lose_next_success_response = false;
                return Err(DocumentStoreError(
                    "commit applied but success response was lost".into(),
                ));
            }
            Ok(committed)
        }
    }

    #[test]
    fn lost_success_response_is_reconciled_from_atomic_persisted_fact() {
        let (source, id) = snapshot_with_outbox("p");
        let store = ApplyThenLoseResponseStore {
            inner: MemoryDocumentStore::default(),
            lose_next_success_response: false,
        };
        let mut repository = AggregateDocumentRepository::new(store);
        repository.create(&source).unwrap();

        let claim = crate::kernel::DeliveryClaim {
            owner: "worker".into(),
            fence: 1,
            lease_until_ms: 100,
            heartbeat_revision: 0,
        };
        repository.store.lose_next_success_response = true;
        assert!(matches!(
            OutboxRepository::evolve_outbox(
                &mut repository,
                &id,
                0,
                OutboxEvent::Claim {
                    claim: claim.clone(),
                    now_ms: 0,
                },
            ),
            Err(OutboxRepositoryError::Unavailable(_))
        ));

        let loaded = AggregateDocumentRepository::load(&repository, "p")
            .unwrap()
            .unwrap();
        let persisted = loaded.snapshot.outbox.get(&id).unwrap();
        assert_eq!(persisted.revision, 1);
        assert_eq!(
            persisted.state,
            crate::kernel::OutboxState::Claimed(claim.clone())
        );
        let index = OutboxIndexCodec::decode(loaded.original_outbox_index().unwrap()).unwrap();
        assert_eq!(index.entries.len(), 1);
        assert_eq!(index.entries[0].id, id);
        assert_eq!(index.entries[0].revision, 1);

        assert!(matches!(
            OutboxRepository::evolve_outbox(
                &mut repository,
                &id,
                0,
                OutboxEvent::Claim { claim, now_ms: 0 },
            ),
            Err(OutboxRepositoryError::RevisionConflict {
                id: conflict_id,
                expected: 0,
                actual: 1,
            }) if conflict_id == id
        ));
        let after_retry = AggregateDocumentRepository::load(&repository, "p")
            .unwrap()
            .unwrap();
        assert_eq!(after_retry.snapshot, loaded.snapshot);
        assert_eq!(
            after_retry.original_outbox_index(),
            loaded.original_outbox_index()
        );
    }
}
