use crate::scheduler::{
    core::{
        commit::aggregate::ProcessSnapshot,
        outbox::outbox_index::{OutboxIndex, OutboxIndexCodec},
        ready::{ReadyIndex, ReadyIndexCodec},
        recovery::{
            recovery_entry::{RECOVERY_ENTRY_KEY_PREFIX, RecoveryEntryDocument},
            recovery_index::{RecoveryIndex, RecoveryIndexCodec},
        },
    },
    durable::document::{ProcessDocumentCodec, ProcessDocumentError},
};

use std::collections::BTreeMap;
use std::fmt;

pub(crate) const PROCESS_KEY_PREFIX: &str = "kernel/aggregates/";
pub(crate) const RECOVERY_INDEX_KEY_PREFIX: &str = "kernel/indexes/recovery/";
pub(crate) const OUTBOX_INDEX_KEY_PREFIX: &str = "kernel/indexes/outbox/";
pub(crate) const READY_INDEX_KEY_PREFIX: &str = "kernel/indexes/ready/";

/// The exact documents one guarded replacement wrote. The caller adopts them
/// as its next CAS token instead of re-reading or re-encoding.
#[derive(Debug)]
pub(crate) struct ReplaceWrite {
    pub(crate) document: Vec<u8>,
    pub(crate) recovery_index: Option<Vec<u8>>,
    pub(crate) outbox_index: Option<Vec<u8>>,
    pub(crate) ready: Option<Vec<u8>>,
}

/// A loaded aggregate and the exact bytes observed from storage. The raw bytes
/// are the CAS token. Re-encoding the snapshot is forbidden because a future
/// codec may normalize legacy documents into different bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LoadedProcess {
    pub(crate) snapshot: ProcessSnapshot,
    original_document: Vec<u8>,
    original_recovery_index: Option<Vec<u8>>,
    original_outbox_index: Option<Vec<u8>>,
    original_ready_index: Option<Vec<u8>>,
}

impl LoadedProcess {
    pub(crate) fn original_document(&self) -> &[u8] {
        &self.original_document
    }
    pub(crate) fn original_recovery_index(&self) -> Option<&[u8]> {
        self.original_recovery_index.as_deref()
    }
    pub(crate) fn original_outbox_index(&self) -> Option<&[u8]> {
        self.original_outbox_index.as_deref()
    }
    pub(crate) fn original_ready_index(&self) -> Option<&[u8]> {
        self.original_ready_index.as_deref()
    }
    /// Adopt the exact bytes a replacement just wrote, so the next guarded
    /// write uses them as the CAS token without re-reading or re-encoding.
    pub(crate) fn install_replace_documents(&mut self, write: ReplaceWrite) {
        self.original_document = write.document;
        self.original_recovery_index = write.recovery_index;
        self.original_outbox_index = write.outbox_index;
        self.original_ready_index = write.ready;
    }
    /// Adopt the exact bytes a confirmation just wrote for the aggregate and
    /// its outbox index. The recovery and ready indexes cannot change in that
    /// batch, so their observed bytes stay the CAS token.
    pub(crate) fn install_confirmed_documents(
        &mut self,
        document: Vec<u8>,
        outbox_index: Option<Vec<u8>>,
    ) {
        self.original_document = document;
        self.original_outbox_index = outbox_index;
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

pub(crate) fn outbox_index_key(process_id: &str) -> Result<String, DocumentRepositoryError> {
    encoded_process_key(OUTBOX_INDEX_KEY_PREFIX, process_id)
}

pub(crate) fn outbox_index_document(
    snapshot: &ProcessSnapshot,
) -> Result<Option<Vec<u8>>, DocumentRepositoryError> {
    OutboxIndex::project(&snapshot.process_id, &snapshot.outbox)
        .map_err(|error| DocumentRepositoryError::OutboxIndex(error.to_string()))?
        .map(|index| {
            OutboxIndexCodec::encode(&index)
                .map_err(|error| DocumentRepositoryError::OutboxIndex(error.to_string()))
        })
        .transpose()
}

pub(crate) fn recovery_index_key(process_id: &str) -> Result<String, DocumentRepositoryError> {
    encoded_process_key(RECOVERY_INDEX_KEY_PREFIX, process_id)
}

pub(crate) fn recovery_index_document(
    snapshot: &ProcessSnapshot,
) -> Result<Option<Vec<u8>>, DocumentRepositoryError> {
    RecoveryIndex::project(snapshot)
        .map(|index| {
            RecoveryIndexCodec::encode(&index)
                .map_err(|error| DocumentRepositoryError::RecoveryIndex(error.to_string()))
        })
        .transpose()
}

fn legacy_recovery_index_document(
    snapshot: &ProcessSnapshot,
) -> Result<Option<Vec<u8>>, DocumentRepositoryError> {
    RecoveryIndex::project_legacy(snapshot)
        .map(|index| {
            RecoveryIndexCodec::encode(&index)
                .map_err(|error| DocumentRepositoryError::RecoveryIndex(error.to_string()))
        })
        .transpose()
}

pub(crate) fn ready_index_key(process_id: &str) -> Result<String, DocumentRepositoryError> {
    encoded_process_key(READY_INDEX_KEY_PREFIX, process_id)
}

fn ready_index_document(
    snapshot: &ProcessSnapshot,
    now_ms: i64,
    poll_ms: i64,
) -> Result<Option<Vec<u8>>, DocumentRepositoryError> {
    ReadyIndex::project(snapshot, now_ms, poll_ms)
        .map(|index| {
            ReadyIndexCodec::encode(&index)
                .map_err(|error| DocumentRepositoryError::ReadyIndex(error.to_string()))
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

/// The documents one atomic install writes: the aggregate snapshot bytes plus
/// the replacement recovery-index, outbox-index and ready-index documents.
/// Shared by the replace and create transaction builders.
pub(crate) type TransactionParts = Result<
    (
        DocumentTransaction,
        Vec<u8>,
        Option<Vec<u8>>,
        Option<Vec<u8>>,
        Option<Vec<u8>>,
    ),
    DocumentRepositoryError,
>;

/// The recovery-entry documents one snapshot projects.
///
/// A repository that advances its loaded aggregate in place must capture these
/// *before* the advance: the CAS guards for the entry documents compare against
/// what the store currently holds, which is the pre-advance projection.
pub(crate) fn recovery_entries(snapshot: &ProcessSnapshot) -> BTreeMap<String, Vec<u8>> {
    RecoveryEntryDocument::project(snapshot)
}

pub(crate) fn build_replace_transaction(
    loaded: &LoadedProcess,
    replacement_snapshot: &ProcessSnapshot,
    current_recovery_entries: &BTreeMap<String, Vec<u8>>,
    replacement_ready: Option<&[u8]>,
) -> TransactionParts {
    if loaded.snapshot.process_id != replacement_snapshot.process_id {
        return Err(DocumentRepositoryError::ProcessIdentityMismatch {
            requested: loaded.snapshot.process_id.clone(),
            stored: replacement_snapshot.process_id.clone(),
        });
    }
    let process_id = &loaded.snapshot.process_id;
    let key = process_key(process_id)?;
    let recovery_key = recovery_index_key(process_id)?;
    let outbox_key = outbox_index_key(process_id)?;
    let ready_key = ready_index_key(process_id)?;
    let document = ProcessDocumentCodec::encode(replacement_snapshot)?;
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
    transaction = match loaded.original_ready_index() {
        Some(index) => transaction.require_exact(&ready_key, index.to_vec())?,
        None => transaction.require_absent(&ready_key)?,
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
    transaction = match replacement_ready {
        Some(index) => transaction.put(&ready_key, index.to_vec())?,
        None => transaction.delete(&ready_key)?,
    };
    transaction =
        replace_recovery_entries(transaction, current_recovery_entries, replacement_snapshot)?;
    transaction.validate()?;
    Ok((
        transaction,
        document,
        recovery_index,
        outbox_index,
        replacement_ready.map(ToOwned::to_owned),
    ))
}

/// The guarded batch for one operation's post-delivery confirmation. Only the
/// aggregate and its outbox index can move: the confirmation advances outbox
/// records without changing tasks, operations or variables, so the recovery
/// index (and its per-operation entry documents) and the ready index are
/// guarded but not rewritten. Rewriting an unchanged derived index would make
/// the normal path pay for work that cannot have changed.
pub(crate) struct OutboxConfirmWrite {
    pub(crate) transaction: DocumentTransaction,
    /// The exact aggregate bytes the batch writes, kept so the caller can
    /// adopt them as the next CAS token without re-encoding.
    pub(crate) document: Vec<u8>,
    pub(crate) outbox_index: Option<Vec<u8>>,
}

pub(crate) fn build_outbox_confirm_transaction(
    loaded: &LoadedProcess,
    replacement: &ProcessSnapshot,
) -> Result<OutboxConfirmWrite, DocumentRepositoryError> {
    if loaded.snapshot.process_id != replacement.process_id {
        return Err(DocumentRepositoryError::ProcessIdentityMismatch {
            requested: loaded.snapshot.process_id.clone(),
            stored: replacement.process_id.clone(),
        });
    }
    let process_id = &loaded.snapshot.process_id;
    let key = process_key(process_id)?;
    let recovery_key = recovery_index_key(process_id)?;
    let outbox_key = outbox_index_key(process_id)?;
    let ready_key = ready_index_key(process_id)?;
    let document = ProcessDocumentCodec::encode(replacement)?;
    let outbox_index = outbox_index_document(replacement)?;
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
    transaction = match loaded.original_ready_index() {
        Some(index) => transaction.require_exact(&ready_key, index.to_vec())?,
        None => transaction.require_absent(&ready_key)?,
    };
    transaction = transaction.put(&key, document.clone())?;
    transaction = match &outbox_index {
        Some(index) => transaction.put(&outbox_key, index.clone())?,
        None => transaction.delete(&outbox_key)?,
    };
    transaction.validate()?;
    Ok(OutboxConfirmWrite {
        transaction,
        document,
        outbox_index,
    })
}

fn replace_recovery_entries(
    mut transaction: DocumentTransaction,
    current_entries: &BTreeMap<String, Vec<u8>>,
    replacement: &ProcessSnapshot,
) -> Result<DocumentTransaction, DocumentStoreError> {
    let replacement_entries = RecoveryEntryDocument::project(replacement);
    for (key, value) in current_entries {
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

pub(crate) fn build_create_transaction(
    snapshot: &ProcessSnapshot,
    now_ms: i64,
    poll_ms: i64,
) -> TransactionParts {
    let key = process_key(&snapshot.process_id)?;
    let recovery_key = recovery_index_key(&snapshot.process_id)?;
    let outbox_key = outbox_index_key(&snapshot.process_id)?;
    let ready_key = ready_index_key(&snapshot.process_id)?;
    let document = ProcessDocumentCodec::encode(snapshot)?;
    let recovery_index = recovery_index_document(snapshot)?;
    let outbox_index = outbox_index_document(snapshot)?;
    let ready_index = ready_index_document(snapshot, now_ms, poll_ms)?;
    let mut transaction = DocumentTransaction::new()
        .require_absent(&key)?
        .require_absent(&recovery_key)?
        .require_absent(&outbox_key)?
        .require_absent(&ready_key)?
        .put(&key, document.clone())?;
    transaction = match &recovery_index {
        Some(index) => transaction.put(&recovery_key, index.clone())?,
        None => transaction.delete(&recovery_key)?,
    };
    transaction = match &outbox_index {
        Some(index) => transaction.put(&outbox_key, index.clone())?,
        None => transaction.delete(&outbox_key)?,
    };
    transaction = match &ready_index {
        Some(index) => transaction.put(&ready_key, index.clone())?,
        None => transaction.delete(&ready_key)?,
    };
    for (entry_key, entry_value) in RecoveryEntryDocument::project(snapshot) {
        transaction = transaction
            .require_absent(entry_key.as_str())?
            .put(entry_key, entry_value)?;
    }
    transaction.validate()?;
    Ok((
        transaction,
        document,
        recovery_index,
        outbox_index,
        ready_index,
    ))
}

/// The derived-index repair of one loaded aggregate: the normalized recovery
/// index and ready index, guarded by the exact documents the repair read.
/// `None` means both already match and nothing must be written.
pub(crate) fn build_repair_transaction(
    loaded: &LoadedProcess,
    recovery_index: Option<&[u8]>,
    ready_index: Option<&[u8]>,
) -> Result<Option<DocumentTransaction>, DocumentRepositoryError> {
    let process_id = &loaded.snapshot.process_id;
    let key = process_key(process_id)?;
    let recovery_key = recovery_index_key(process_id)?;
    let outbox_key = outbox_index_key(process_id)?;
    let ready_key = ready_index_key(process_id)?;
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
    transaction = match loaded.original_ready_index() {
        Some(index) => transaction.require_exact(&ready_key, index.to_vec())?,
        None => transaction.require_absent(&ready_key)?,
    };
    let mut changed = false;
    if recovery_index != loaded.original_recovery_index() {
        transaction = match recovery_index {
            Some(index) => transaction.put(&recovery_key, index.to_vec())?,
            None => transaction.delete(&recovery_key)?,
        };
        changed = true;
    }
    if ready_index != loaded.original_ready_index() {
        transaction = match ready_index {
            Some(index) => transaction.put(&ready_key, index.to_vec())?,
            None => transaction.delete(&ready_key)?,
        };
        changed = true;
    }
    if !changed {
        return Ok(None);
    }
    transaction.validate()?;
    Ok(Some(transaction))
}

/// The scan prefix of one process's per-operation recovery-entry documents.
///
/// The entry key is `{prefix}{hex(pid)}.{wire}.{sequence}`; the trailing dot
/// keeps a pid from matching a longer pid that shares its hex prefix.
pub(crate) fn recovery_entry_process_prefix(
    process_id: &str,
) -> Result<String, DocumentRepositoryError> {
    Ok(format!(
        "{}.",
        encoded_process_key(RECOVERY_ENTRY_KEY_PREFIX, process_id)?
    ))
}

/// The authoritative deletion of a reclaimable run: the aggregate plus its
/// recovery, outbox and ready index documents, guarded by the exact bytes the
/// decision was taken on (the aggregate document bytes are the version guard:
/// any commit installs different bytes).
///
/// `recovery_entries` are the per-operation entry documents found under the
/// process's prefix. A reclaimable snapshot projects none, so any found are
/// leftovers from an older database; they are deleted in the same batch so no
/// index can outlive the aggregate.
pub(crate) fn build_reclaim_transaction(
    loaded: &LoadedProcess,
    recovery_entries: &[(String, Vec<u8>)],
) -> Result<DocumentTransaction, DocumentRepositoryError> {
    let process_id = &loaded.snapshot.process_id;
    let key = process_key(process_id)?;
    let recovery_key = recovery_index_key(process_id)?;
    let outbox_key = outbox_index_key(process_id)?;
    let ready_key = ready_index_key(process_id)?;
    let mut transaction =
        DocumentTransaction::new().require_exact(&key, loaded.original_document().to_vec())?;
    transaction =
        guard_original_document(transaction, &recovery_key, loaded.original_recovery_index())?;
    transaction =
        guard_original_document(transaction, &outbox_key, loaded.original_outbox_index())?;
    transaction = guard_original_document(transaction, &ready_key, loaded.original_ready_index())?;
    for (entry_key, entry_value) in recovery_entries {
        transaction = transaction.require_exact(entry_key.as_str(), entry_value.clone())?;
    }
    transaction = transaction
        .delete(&key)?
        .delete(&recovery_key)?
        .delete(&outbox_key)?
        .delete(&ready_key)?;
    for (entry_key, _) in recovery_entries {
        transaction = transaction.delete(entry_key.as_str())?;
    }
    transaction.validate()?;
    Ok(transaction)
}

/// The same deletion when the aggregate is already gone (an interrupted
/// cleanup): only the derived indexes found in storage are removed. The
/// aggregate key is guarded absent, so a concurrent start that recreates the
/// pid wins and the caller re-reads instead of deleting the new run's indexes.
pub(crate) fn build_orphan_reclaim_transaction(
    process_id: &str,
    recovery_index: Option<Vec<u8>>,
    outbox_index: Option<Vec<u8>>,
    ready_index: Option<Vec<u8>>,
    recovery_entries: &[(String, Vec<u8>)],
) -> Result<DocumentTransaction, DocumentRepositoryError> {
    let key = process_key(process_id)?;
    let recovery_key = recovery_index_key(process_id)?;
    let outbox_key = outbox_index_key(process_id)?;
    let ready_key = ready_index_key(process_id)?;
    let mut transaction = DocumentTransaction::new().require_absent(&key)?;
    transaction = guard_original_document(transaction, &recovery_key, recovery_index.as_deref())?;
    transaction = guard_original_document(transaction, &outbox_key, outbox_index.as_deref())?;
    transaction = guard_original_document(transaction, &ready_key, ready_index.as_deref())?;
    for (entry_key, entry_value) in recovery_entries {
        transaction = transaction.require_exact(entry_key.as_str(), entry_value.clone())?;
    }
    transaction = transaction
        .delete(&key)?
        .delete(&recovery_key)?
        .delete(&outbox_key)?
        .delete(&ready_key)?;
    for (entry_key, _) in recovery_entries {
        transaction = transaction.delete(entry_key.as_str())?;
    }
    transaction.validate()?;
    Ok(transaction)
}

fn guard_original_document(
    transaction: DocumentTransaction,
    key: &str,
    expected: Option<&[u8]>,
) -> Result<DocumentTransaction, DocumentStoreError> {
    match expected {
        Some(bytes) => transaction.require_exact(key, bytes.to_vec()),
        None => transaction.require_absent(key),
    }
}

pub(crate) fn loaded_after_write(
    snapshot: &ProcessSnapshot,
    document: Vec<u8>,
    recovery_index: Option<Vec<u8>>,
    outbox_index: Option<Vec<u8>>,
    ready_index: Option<Vec<u8>>,
) -> LoadedProcess {
    LoadedProcess {
        snapshot: snapshot.clone(),
        original_document: document,
        original_recovery_index: recovery_index,
        original_outbox_index: outbox_index,
        original_ready_index: ready_index,
    }
}

pub(crate) fn decode_loaded_process(
    process_id: &str,
    original_document: Vec<u8>,
    original_recovery_index: Option<Vec<u8>>,
    original_outbox_index: Option<Vec<u8>>,
    original_ready_index: Option<Vec<u8>>,
) -> Result<LoadedProcess, DocumentRepositoryError> {
    let snapshot = ProcessDocumentCodec::decode(&original_document).map_err(|source| {
        DocumentRepositoryError::CorruptProcess {
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
    let legacy_recovery_index = legacy_recovery_index_document(&snapshot)?;
    if original_recovery_index.as_deref() != expected_recovery_index.as_deref()
        && original_recovery_index.as_deref() != legacy_recovery_index.as_deref()
    {
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
    Ok(LoadedProcess {
        snapshot,
        original_document,
        original_recovery_index,
        original_outbox_index,
        original_ready_index,
    })
}

pub(crate) fn process_key(process_id: &str) -> Result<String, DocumentRepositoryError> {
    encoded_process_key(PROCESS_KEY_PREFIX, process_id)
}

/// The process id behind one encoded aggregate key.
pub(crate) fn decode_process_key(key: &str) -> Result<String, DocumentRepositoryError> {
    let encoded = key
        .strip_prefix(PROCESS_KEY_PREFIX)
        .ok_or_else(|| DocumentRepositoryError::KeyEncoding(format!("foreign key {key:?}")))?;
    if encoded.len() % 2 != 0 {
        return Err(DocumentRepositoryError::KeyEncoding(format!(
            "odd-length process key {key:?}"
        )));
    }
    let mut bytes = Vec::with_capacity(encoded.len() / 2);
    let (pairs, _) = encoded.as_bytes().as_chunks::<2>();
    for pair in pairs {
        let high = hex_value(pair[0]).ok_or_else(|| {
            DocumentRepositoryError::KeyEncoding(format!("non-hex process key {key:?}"))
        })?;
        let low = hex_value(pair[1]).ok_or_else(|| {
            DocumentRepositoryError::KeyEncoding(format!("non-hex process key {key:?}"))
        })?;
        bytes.push((high << 4) | low);
    }
    String::from_utf8(bytes)
        .map_err(|_| DocumentRepositoryError::KeyEncoding(format!("non-utf8 process key {key:?}")))
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DocumentRepositoryError {
    EmptyProcessId,
    KeyEncoding(String),
    ProcessIdentityMismatch {
        requested: String,
        stored: String,
    },
    CorruptProcess {
        process_id: String,
        source: ProcessDocumentError,
    },
    Codec(ProcessDocumentError),
    RecoveryIndex(String),
    RecoveryIndexMismatch {
        process_id: String,
    },
    ReadyIndex(String),
    OutboxIndex(String),
    OutboxIndexMismatch {
        process_id: String,
    },
    Store(DocumentStoreError),
}
impl From<ProcessDocumentError> for DocumentRepositoryError {
    fn from(error: ProcessDocumentError) -> Self {
        Self::Codec(error)
    }
}

impl DocumentRepositoryError {
    /// Whether this error means "the read raced a writer" rather than damage:
    /// the aggregate and its derived indexes were read at different versions
    /// (they are read key by key), so the trio is internally inconsistent while
    /// every document in it is intact. Re-reading resolves it.
    pub(crate) fn is_read_race(&self) -> bool {
        matches!(
            self,
            Self::RecoveryIndexMismatch { .. } | Self::OutboxIndexMismatch { .. }
        )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_key_is_unambiguous_for_separator_characters() {
        assert_ne!(process_key("a/b").unwrap(), process_key("a:2fb").unwrap());
        assert_eq!(process_key("é").unwrap(), "kernel/aggregates/c3a9");
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
}
