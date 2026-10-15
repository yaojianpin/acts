use crate::scheduler::{
    core::{
        commit::process::ProcessSnapshot,
        outbox::outbox_index::{OutboxIndex, OutboxIndexCodec},
        recovery::{
            recovery_entry::RecoveryEntryDocument,
            recovery_index::{RecoveryIndex, RecoveryIndexCodec},
        },
    },
    durable::document::{ProcessDocumentCodec, ProcessDocumentError},
};

use std::collections::BTreeMap;
use std::fmt;

const PROCESS_KEY_PREFIX: &str = "kernel/aggregates/";
pub(crate) const RECOVERY_INDEX_KEY_PREFIX: &str = "kernel/indexes/recovery/";
pub(crate) const OUTBOX_INDEX_KEY_PREFIX: &str = "kernel/indexes/outbox/";

/// A loaded aggregate and the exact bytes observed from storage. The raw bytes
/// are the CAS token. Re-encoding the snapshot is forbidden because a future
/// codec may normalize legacy documents into different bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LoadedProcess {
    pub(crate) snapshot: ProcessSnapshot,
    original_document: Vec<u8>,
    original_recovery_index: Option<Vec<u8>>,
    original_outbox_index: Option<Vec<u8>>,
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

fn outbox_index_document(
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

fn recovery_index_document(
    snapshot: &ProcessSnapshot,
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
pub(crate) type TransactionParts = Result<
    (
        DocumentTransaction,
        Vec<u8>,
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
    transaction = transaction.put(&key, document.clone())?;
    transaction = match &recovery_index {
        Some(index) => transaction.put(&recovery_key, index.clone())?,
        None => transaction.delete(&recovery_key)?,
    };
    transaction = match &outbox_index {
        Some(index) => transaction.put(&outbox_key, index.clone())?,
        None => transaction.delete(&outbox_key)?,
    };
    transaction =
        replace_recovery_entries(transaction, current_recovery_entries, replacement_snapshot)?;
    transaction.validate()?;
    Ok((transaction, document, recovery_index, outbox_index))
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

pub(crate) fn build_create_transaction(snapshot: &ProcessSnapshot) -> TransactionParts {
    let key = process_key(&snapshot.process_id)?;
    let recovery_key = recovery_index_key(&snapshot.process_id)?;
    let outbox_key = outbox_index_key(&snapshot.process_id)?;
    let document = ProcessDocumentCodec::encode(snapshot)?;
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

pub(crate) fn loaded_after_write(
    snapshot: &ProcessSnapshot,
    document: Vec<u8>,
    recovery_index: Option<Vec<u8>>,
    outbox_index: Option<Vec<u8>>,
) -> LoadedProcess {
    LoadedProcess {
        snapshot: snapshot.clone(),
        original_document: document,
        original_recovery_index: recovery_index,
        original_outbox_index: outbox_index,
    }
}

pub(crate) fn decode_loaded_process(
    process_id: &str,
    original_document: Vec<u8>,
    original_recovery_index: Option<Vec<u8>>,
    original_outbox_index: Option<Vec<u8>>,
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
    Ok(LoadedProcess {
        snapshot,
        original_document,
        original_recovery_index,
        original_outbox_index,
    })
}

pub(crate) fn process_key(process_id: &str) -> Result<String, DocumentRepositoryError> {
    encoded_process_key(PROCESS_KEY_PREFIX, process_id)
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
