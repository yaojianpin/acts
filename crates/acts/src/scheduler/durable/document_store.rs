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
    durable::{
        document::{ProcessDocumentCodec, ProcessDocumentError, ProcessLogCodec},
        envelope::{
            AGGREGATE_SCHEMA_VERSION, EnvelopeError, LEGACY_AGGREGATE_SCHEMA_VERSION,
            ProcessEnvelope,
        },
        snapshot_codec::{HeadPayload, LogChanges, SnapshotCodecError},
    },
};

use std::collections::BTreeMap;
use std::fmt;

pub(crate) const PROCESS_KEY_PREFIX: &str = "kernel/aggregates/";
pub(crate) const RECOVERY_INDEX_KEY_PREFIX: &str = "kernel/indexes/recovery/";
pub(crate) const OUTBOX_INDEX_KEY_PREFIX: &str = "kernel/indexes/outbox/";
pub(crate) const READY_INDEX_KEY_PREFIX: &str = "kernel/indexes/ready/";
/// Under the head document, the base snapshot and the append-only log are
/// namespaced by these segments. A head key never contains a `/` after the
/// prefix, so the process census can tell the family's head from its parts.
const PROCESS_BASE_SEGMENT: &str = "base";
const PROCESS_LOG_SEGMENT: &str = "log";
/// How many log entries may stand between two base rewrites. A normal commit
/// appends one entry; the commit that would exceed the window rebases the
/// snapshot and deletes the window in the same atomic batch.
pub(crate) const MAX_LOG_ENTRIES: u64 = 32;

/// The exact documents one guarded replacement wrote. The caller adopts them
/// as its next CAS token instead of re-reading or re-encoding.
#[derive(Debug)]
pub(crate) struct ReplaceWrite {
    pub(crate) document: Vec<u8>,
    pub(crate) base: Option<Vec<u8>>,
    pub(crate) log_entry: Option<(u64, Vec<u8>)>,
    pub(crate) epoch: u64,
    pub(crate) base_seq: u64,
    pub(crate) next_seq: u64,
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
    original_base: Option<Vec<u8>>,
    /// Log entries keyed by sequence, with the exact bytes they were read as.
    original_log: BTreeMap<u64, Vec<u8>>,
    epoch: u64,
    base_seq: u64,
    next_seq: u64,
    legacy: bool,
    original_recovery_index: Option<Vec<u8>>,
    original_outbox_index: Option<Vec<u8>>,
    original_ready_index: Option<Vec<u8>>,
}

impl LoadedProcess {
    pub(crate) fn original_document(&self) -> &[u8] {
        &self.original_document
    }
    pub(crate) fn original_base(&self) -> Option<&[u8]> {
        self.original_base.as_deref()
    }
    pub(crate) fn original_log(&self) -> &BTreeMap<u64, Vec<u8>> {
        &self.original_log
    }
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }
    pub(crate) fn base_seq(&self) -> u64 {
        self.base_seq
    }
    pub(crate) fn next_seq(&self) -> u64 {
        self.next_seq
    }
    /// Whether this aggregate was loaded from a pre-L8 whole-snapshot
    /// document and still needs its base snapshot written.
    pub(crate) fn is_legacy(&self) -> bool {
        self.legacy
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
        if let Some(base) = write.base {
            self.original_base = Some(base);
            self.original_log.clear();
        }
        if let Some((sequence, entry)) = write.log_entry {
            self.original_log.insert(sequence, entry);
        }
        self.epoch = write.epoch;
        self.base_seq = write.base_seq;
        self.next_seq = write.next_seq;
        self.legacy = false;
        self.original_recovery_index = write.recovery_index;
        self.original_outbox_index = write.outbox_index;
        self.original_ready_index = write.ready;
    }
    /// Adopt the exact bytes a confirmation just wrote for the aggregate and
    /// its outbox index. The recovery and ready indexes cannot change in that
    /// batch, so their observed bytes stay the CAS token.
    pub(crate) fn install_confirmed_documents(&mut self, write: ReplaceWrite) {
        self.original_document = write.document;
        if let Some(base) = write.base {
            self.original_base = Some(base);
            self.original_log.clear();
        }
        if let Some((sequence, entry)) = write.log_entry {
            self.original_log.insert(sequence, entry);
        }
        self.epoch = write.epoch;
        self.base_seq = write.base_seq;
        self.next_seq = write.next_seq;
        self.legacy = false;
        self.original_outbox_index = write.outbox_index;
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

/// One atomic aggregate install: the guarded transaction plus the exact bytes
/// it writes, so the caller can adopt them as its next CAS token.
pub(crate) struct InstallWrite {
    pub(crate) transaction: DocumentTransaction,
    pub(crate) write: ReplaceWrite,
}

/// The recovery-entry documents one snapshot projects.
///
/// A repository that advances its loaded aggregate in place must capture these
/// *before* the advance: the CAS guards for the entry documents compare against
/// what the store currently holds, which is the pre-advance projection.
pub(crate) fn recovery_entries(snapshot: &ProcessSnapshot) -> BTreeMap<String, Vec<u8>> {
    RecoveryEntryDocument::project(snapshot)
}

fn next_counter(value: u64) -> Result<u64, DocumentRepositoryError> {
    value
        .checked_add(1)
        .ok_or_else(|| DocumentStoreError("aggregate sequence or epoch overflow".into()).into())
}

/// Plan the aggregate part of one commit.
///
/// The normal path appends one log entry that encodes only `changes` and
/// writes a fresh head; the base snapshot is untouched. When the log window is
/// full the same transaction instead writes a new base for `replacement` and
/// deletes the folded entries. A loaded legacy document (or one whose
/// migration could not be written) is rebased the same way, which is how a
/// first commit after an old database opened upgrades it.
fn install_aggregate(
    loaded: &LoadedProcess,
    replacement: &ProcessSnapshot,
    changes: &LogChanges,
    transaction: DocumentTransaction,
) -> Result<(DocumentTransaction, ReplaceWrite), DocumentRepositoryError> {
    let process_id = &loaded.snapshot.process_id;
    let key = process_key(process_id)?;
    let base_key = process_base_key(process_id)?;
    let epoch = next_counter(loaded.epoch())?;
    let mut transaction = transaction;
    // Deferred mutations: the head is always the first aggregate mutation, so
    // a fault aimed at the aggregate's exact key lands on the commit itself.
    let mut after_head: Vec<(String, Option<Vec<u8>>)> = Vec::new();
    let (base, log_entry, base_seq, next_seq) =
        if loaded.is_legacy() || loaded.original_base().is_none() {
            // Rebase: the legacy (or partial) documents carry no base snapshot
            // this format can append to.
            let base = ProcessLogCodec::encode_base(replacement, 0)?;
            transaction = match loaded.original_base() {
                Some(previous) => transaction.require_exact(&base_key, previous.to_vec())?,
                None => transaction.require_absent(&base_key)?,
            };
            after_head.push((base_key.clone(), Some(base.clone())));
            (Some(base), None, 0, 0)
        } else if loaded.next_seq().saturating_sub(loaded.base_seq()) >= MAX_LOG_ENTRIES {
            // Compaction: fold the whole window into a fresh base and delete
            // the entries in the same guarded batch.
            let sequence = loaded.next_seq();
            let base = ProcessLogCodec::encode_base(replacement, sequence)?;
            let previous_base = loaded
                .original_base()
                .ok_or_else(|| DocumentStoreError("compaction requires a base snapshot".into()))?;
            transaction = transaction.require_exact(&base_key, previous_base.to_vec())?;
            for (log_sequence, bytes) in loaded.original_log() {
                let entry_key = process_log_key(process_id, *log_sequence)?;
                transaction = transaction.require_exact(&entry_key, bytes.clone())?;
                after_head.push((entry_key, None));
            }
            after_head.push((base_key.clone(), Some(base.clone())));
            (Some(base), None, sequence, sequence)
        } else {
            let sequence = loaded.next_seq();
            let entry = ProcessLogCodec::encode_log(replacement, sequence, changes)?;
            let entry_key = process_log_key(process_id, sequence)?;
            transaction = transaction.require_absent(&entry_key)?;
            after_head.push((entry_key, Some(entry.clone())));
            (
                None,
                Some((sequence, entry)),
                loaded.base_seq(),
                next_counter(sequence)?,
            )
        };
    let head = ProcessLogCodec::encode_head(&HeadPayload {
        process_id: process_id.clone(),
        version: replacement.version.clone(),
        epoch,
        base_seq,
        next_seq,
    })?;
    transaction = transaction.put(&key, head.clone())?;
    for (mutation_key, value) in after_head {
        transaction = match value {
            Some(value) => transaction.put(mutation_key, value)?,
            None => transaction.delete(mutation_key)?,
        };
    }
    transaction.validate()?;
    Ok((
        transaction,
        ReplaceWrite {
            document: head,
            base,
            log_entry,
            epoch,
            base_seq,
            next_seq,
            recovery_index: None,
            outbox_index: None,
            ready: None,
        },
    ))
}

fn index_guards(
    transaction: DocumentTransaction,
    loaded: &LoadedProcess,
) -> Result<DocumentTransaction, DocumentRepositoryError> {
    let process_id = &loaded.snapshot.process_id;
    let recovery_key = recovery_index_key(process_id)?;
    let outbox_key = outbox_index_key(process_id)?;
    let ready_key = ready_index_key(process_id)?;
    let mut transaction = match loaded.original_recovery_index() {
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
    Ok(transaction)
}

/// Write a derived index only when it actually changed. The guard for the key
/// is already in the transaction, so leaving the mutation out keeps the
/// compare-and-swap contract intact while sparing a backend a write whose
/// bytes are identical to the stored ones (an index key is either present with
/// its exact bytes or absent — there is nothing else to heal here).
fn write_index_if_changed(
    transaction: DocumentTransaction,
    key: &str,
    original: Option<&[u8]>,
    replacement: Option<&[u8]>,
) -> Result<DocumentTransaction, DocumentStoreError> {
    if original == replacement {
        return Ok(transaction);
    }
    match replacement {
        Some(bytes) => transaction.put(key, bytes.to_vec()),
        None => transaction.delete(key),
    }
}

pub(crate) fn build_replace_transaction(
    loaded: &LoadedProcess,
    replacement_snapshot: &ProcessSnapshot,
    current_recovery_entries: &BTreeMap<String, Vec<u8>>,
    replacement_ready: Option<&[u8]>,
    changes: &LogChanges,
) -> Result<InstallWrite, DocumentRepositoryError> {
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
    let recovery_index = recovery_index_document(replacement_snapshot)?;
    let outbox_index = outbox_index_document(replacement_snapshot)?;
    let mut transaction =
        DocumentTransaction::new().require_exact(&key, loaded.original_document().to_vec())?;
    transaction = index_guards(transaction, loaded)?;
    let (mut transaction, mut write) =
        install_aggregate(loaded, replacement_snapshot, changes, transaction)?;
    write.recovery_index = recovery_index;
    write.outbox_index = outbox_index;
    write.ready = replacement_ready.map(ToOwned::to_owned);
    transaction = write_index_if_changed(
        transaction,
        &recovery_key,
        loaded.original_recovery_index(),
        write.recovery_index.as_deref(),
    )?;
    transaction = write_index_if_changed(
        transaction,
        &outbox_key,
        loaded.original_outbox_index(),
        write.outbox_index.as_deref(),
    )?;
    transaction = write_index_if_changed(
        transaction,
        &ready_key,
        loaded.original_ready_index(),
        write.ready.as_deref(),
    )?;
    transaction =
        replace_recovery_entries(transaction, current_recovery_entries, replacement_snapshot)?;
    transaction.validate()?;
    Ok(InstallWrite { transaction, write })
}

/// The guarded batch for one operation's post-delivery confirmation. Only the
/// aggregate and its outbox index can move: the confirmation advances outbox
/// records without changing tasks, operations or variables, so the recovery
/// index (and its per-operation entry documents) and the ready index are
/// guarded but not rewritten. The outbox index itself is written only when its
/// bytes differ from the observed document. Rewriting an unchanged derived
/// index would make the normal path pay for work that cannot have changed.
pub(crate) struct OutboxConfirmWrite {
    pub(crate) transaction: DocumentTransaction,
    pub(crate) write: ReplaceWrite,
}

pub(crate) fn build_outbox_confirm_transaction(
    loaded: &LoadedProcess,
    replacement: &ProcessSnapshot,
    outbox_ids: impl IntoIterator<Item = crate::scheduler::core::outbox::records::OutboxId>,
) -> Result<OutboxConfirmWrite, DocumentRepositoryError> {
    if loaded.snapshot.process_id != replacement.process_id {
        return Err(DocumentRepositoryError::ProcessIdentityMismatch {
            requested: loaded.snapshot.process_id.clone(),
            stored: replacement.process_id.clone(),
        });
    }
    let process_id = &loaded.snapshot.process_id;
    let key = process_key(process_id)?;
    let outbox_key = outbox_index_key(process_id)?;
    let outbox_index = outbox_index_document(replacement)?;
    let mut transaction =
        DocumentTransaction::new().require_exact(&key, loaded.original_document().to_vec())?;
    transaction = index_guards(transaction, loaded)?;
    let changes = LogChanges::outbox(outbox_ids);
    let (mut transaction, mut write) =
        install_aggregate(loaded, replacement, &changes, transaction)?;
    write.outbox_index = outbox_index;
    transaction = write_index_if_changed(
        transaction,
        &outbox_key,
        loaded.original_outbox_index(),
        write.outbox_index.as_deref(),
    )?;
    transaction.validate()?;
    Ok(OutboxConfirmWrite { transaction, write })
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
        // The key's exact bytes are already guarded above; an entry that did
        // not move (another operation's pending revision) needs no write.
        if current_entries.get(&key) == Some(&value) {
            continue;
        }
        transaction = transaction.put(key, value)?;
    }
    Ok(transaction)
}

pub(crate) fn build_create_transaction(
    snapshot: &ProcessSnapshot,
    now_ms: i64,
    poll_ms: i64,
) -> Result<InstallWrite, DocumentRepositoryError> {
    let key = process_key(&snapshot.process_id)?;
    let base_key = process_base_key(&snapshot.process_id)?;
    let recovery_key = recovery_index_key(&snapshot.process_id)?;
    let outbox_key = outbox_index_key(&snapshot.process_id)?;
    let ready_key = ready_index_key(&snapshot.process_id)?;
    let head = ProcessLogCodec::encode_head(&HeadPayload {
        process_id: snapshot.process_id.clone(),
        version: snapshot.version.clone(),
        epoch: 0,
        base_seq: 0,
        next_seq: 0,
    })?;
    let base = ProcessLogCodec::encode_base(snapshot, 0)?;
    let recovery_index = recovery_index_document(snapshot)?;
    let outbox_index = outbox_index_document(snapshot)?;
    let ready_index = ready_index_document(snapshot, now_ms, poll_ms)?;
    let mut transaction = DocumentTransaction::new()
        .require_absent(&key)?
        .require_absent(&base_key)?
        .require_absent(&recovery_key)?
        .require_absent(&outbox_key)?
        .require_absent(&ready_key)?
        .put(&key, head.clone())?
        .put(&base_key, base.clone())?;
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
    Ok(InstallWrite {
        transaction,
        write: ReplaceWrite {
            document: head,
            base: Some(base),
            log_entry: None,
            epoch: 0,
            base_seq: 0,
            next_seq: 0,
            recovery_index,
            outbox_index,
            ready: ready_index,
        },
    })
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
    let base_key = process_base_key(process_id)?;
    let recovery_key = recovery_index_key(process_id)?;
    let outbox_key = outbox_index_key(process_id)?;
    let ready_key = ready_index_key(process_id)?;
    let mut transaction =
        DocumentTransaction::new().require_exact(&key, loaded.original_document().to_vec())?;
    transaction = guard_original_document(transaction, &base_key, loaded.original_base())?;
    for (sequence, bytes) in loaded.original_log() {
        transaction =
            transaction.require_exact(process_log_key(process_id, *sequence)?, bytes.clone())?;
    }
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
        .delete(&base_key)?
        .delete(&recovery_key)?
        .delete(&outbox_key)?
        .delete(&ready_key)?;
    for sequence in loaded.original_log().keys() {
        transaction = transaction.delete(process_log_key(process_id, *sequence)?)?;
    }
    for (entry_key, _) in recovery_entries {
        transaction = transaction.delete(entry_key.as_str())?;
    }
    transaction.validate()?;
    Ok(transaction)
}

/// The same deletion when the aggregate is already gone (an interrupted
/// cleanup): the head is guarded absent, and the base, log entries and derived
/// indexes found in storage are removed. A concurrent start that recreates the
/// pid wins and the caller re-reads instead of deleting the new run.
pub(crate) fn build_orphan_reclaim_transaction(
    process_id: &str,
    base: Option<Vec<u8>>,
    log_entries: &[(String, Vec<u8>)],
    recovery_index: Option<Vec<u8>>,
    outbox_index: Option<Vec<u8>>,
    ready_index: Option<Vec<u8>>,
    recovery_entries: &[(String, Vec<u8>)],
) -> Result<DocumentTransaction, DocumentRepositoryError> {
    let key = process_key(process_id)?;
    let base_key = process_base_key(process_id)?;
    let recovery_key = recovery_index_key(process_id)?;
    let outbox_key = outbox_index_key(process_id)?;
    let ready_key = ready_index_key(process_id)?;
    let mut transaction = DocumentTransaction::new().require_absent(&key)?;
    transaction = guard_original_document(transaction, &base_key, base.as_deref())?;
    for (entry_key, entry_value) in log_entries {
        transaction = transaction.require_exact(entry_key.as_str(), entry_value.clone())?;
    }
    transaction = guard_original_document(transaction, &recovery_key, recovery_index.as_deref())?;
    transaction = guard_original_document(transaction, &outbox_key, outbox_index.as_deref())?;
    transaction = guard_original_document(transaction, &ready_key, ready_index.as_deref())?;
    for (entry_key, entry_value) in recovery_entries {
        transaction = transaction.require_exact(entry_key.as_str(), entry_value.clone())?;
    }
    transaction = transaction
        .delete(&key)?
        .delete(&base_key)?
        .delete(&recovery_key)?
        .delete(&outbox_key)?
        .delete(&ready_key)?;
    for (entry_key, _) in log_entries {
        transaction = transaction.delete(entry_key.as_str())?;
    }
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

/// The transaction that upgrades one legacy whole-snapshot document to the L8
/// family: a fresh head and base, in one guarded batch (the old head bytes are
/// the guard). The derived indexes are bytes the legacy writer produced and
/// stay untouched; a crash before the batch leaves the old document readable,
/// and a retry after it sees the new head.
pub(crate) fn build_migration_transaction(
    loaded: &LoadedProcess,
) -> Result<(DocumentTransaction, Vec<u8>, Vec<u8>), DocumentRepositoryError> {
    if !loaded.is_legacy() {
        return Err(DocumentStoreError("aggregate is already in the L8 format".into()).into());
    }
    let process_id = &loaded.snapshot.process_id;
    let key = process_key(process_id)?;
    let base_key = process_base_key(process_id)?;
    let head = ProcessLogCodec::encode_head(&HeadPayload {
        process_id: process_id.clone(),
        version: loaded.snapshot.version.clone(),
        epoch: 0,
        base_seq: 0,
        next_seq: 0,
    })?;
    let base = ProcessLogCodec::encode_base(&loaded.snapshot, 0)?;
    // The head guard is the transition's version guard: any concurrent writer
    // installs different head bytes. A base document left by an interrupted or
    // older attempt is unreferenced by the legacy head and is replaced rather
    // than guarded, so a first commit after an old database still upgrades.
    let mut transaction =
        DocumentTransaction::new().require_exact(&key, loaded.original_document().to_vec())?;
    transaction = match loaded.original_base() {
        Some(previous) => transaction.require_exact(&base_key, previous.to_vec())?,
        None => transaction,
    };
    for (sequence, bytes) in loaded.original_log() {
        let entry_key = process_log_key(process_id, *sequence)?;
        transaction = transaction
            .require_exact(&entry_key, bytes.clone())?
            .delete(&entry_key)?;
    }
    transaction = transaction
        .put(&key, head.clone())?
        .put(&base_key, base.clone())?;
    transaction.validate()?;
    Ok((transaction, head, base))
}

pub(crate) fn install_migration(
    loaded: LoadedProcess,
    head: Vec<u8>,
    base: Vec<u8>,
) -> LoadedProcess {
    LoadedProcess {
        original_document: head,
        original_base: Some(base),
        original_log: BTreeMap::new(),
        epoch: 0,
        base_seq: 0,
        next_seq: 0,
        legacy: false,
        ..loaded
    }
}

pub(crate) fn loaded_after_write(snapshot: &ProcessSnapshot, write: ReplaceWrite) -> LoadedProcess {
    LoadedProcess {
        snapshot: snapshot.clone(),
        original_document: write.document,
        original_base: write.base,
        original_log: write
            .log_entry
            .map(|(sequence, entry)| BTreeMap::from([(sequence, entry)]))
            .unwrap_or_default(),
        epoch: write.epoch,
        base_seq: write.base_seq,
        next_seq: write.next_seq,
        legacy: false,
        original_recovery_index: write.recovery_index,
        original_outbox_index: write.outbox_index,
        original_ready_index: write.ready,
    }
}

fn corrupt_process(process_id: &str, source: ProcessDocumentError) -> DocumentRepositoryError {
    DocumentRepositoryError::CorruptProcess {
        process_id: process_id.into(),
        source,
    }
}

fn corrupt_log(process_id: &str, message: impl Into<String>) -> DocumentRepositoryError {
    corrupt_process(
        process_id,
        SnapshotCodecError::InvalidLog(message.into()).into(),
    )
}

pub(crate) fn decode_loaded_process(
    process_id: &str,
    original_document: Vec<u8>,
    original_base: Option<Vec<u8>>,
    log_entries: &[(String, Vec<u8>)],
    original_recovery_index: Option<Vec<u8>>,
    original_outbox_index: Option<Vec<u8>>,
    original_ready_index: Option<Vec<u8>>,
) -> Result<LoadedProcess, DocumentRepositoryError> {
    let envelope = ProcessEnvelope::decode(&original_document)
        .map_err(|source| corrupt_process(process_id, source.into()))?;
    let (snapshot, stored_base, stored_log, epoch, base_seq, next_seq, legacy) =
        match envelope.schema_version {
            LEGACY_AGGREGATE_SCHEMA_VERSION => {
                let snapshot = ProcessDocumentCodec::decode_payload(&envelope.payload)
                    .map_err(|source| corrupt_process(process_id, source))?;
                // A pre-L8 head cannot reference a base or log, but an older
                // writer may have left them behind. Retain the observed bytes
                // so migration and reclaim delete the whole family.
                let stale_log = log_entries
                    .iter()
                    .filter_map(|(key, bytes)| {
                        process_log_sequence(process_id, key).map(|seq| (seq, bytes.clone()))
                    })
                    .collect();
                (snapshot, original_base, stale_log, 0, 0, 0, true)
            }
            AGGREGATE_SCHEMA_VERSION => {
                let head = ProcessLogCodec::decode_head_payload(&envelope.payload)
                    .map_err(|source| corrupt_process(process_id, source))?;
                if head.process_id != process_id {
                    return Err(corrupt_log(
                        process_id,
                        format!("head names {:?}", head.process_id),
                    ));
                }
                let base_bytes = original_base
                    .ok_or_else(|| corrupt_log(process_id, "L8 aggregate has no base snapshot"))?;
                let base_envelope = ProcessEnvelope::decode(&base_bytes)
                    .map_err(|source| corrupt_process(process_id, source.into()))?;
                if base_envelope.schema_version != AGGREGATE_SCHEMA_VERSION {
                    return Err(corrupt_log(
                        process_id,
                        format!(
                            "base uses schema {} instead of {AGGREGATE_SCHEMA_VERSION}",
                            base_envelope.schema_version
                        ),
                    ));
                }
                let (base_seq_seen, base) =
                    ProcessLogCodec::decode_base_payload(&base_envelope.payload)
                        .map_err(|source| corrupt_process(process_id, source))?;
                if base_seq_seen != head.base_seq {
                    return Err(corrupt_log(
                        process_id,
                        format!(
                            "base covers sequence {base_seq_seen}, head expects {}",
                            head.base_seq
                        ),
                    ));
                }
                let mut stored_log = BTreeMap::new();
                let mut payloads = Vec::new();
                for (key, bytes) in log_entries {
                    let entry_envelope = ProcessEnvelope::decode(bytes)
                        .map_err(|source| corrupt_process(process_id, source.into()))?;
                    if entry_envelope.schema_version != AGGREGATE_SCHEMA_VERSION {
                        return Err(corrupt_log(
                            process_id,
                            format!("log entry {key:?} uses a foreign schema"),
                        ));
                    }
                    let entry = ProcessLogCodec::decode_log_payload(&entry_envelope.payload)
                        .map_err(|source| corrupt_process(process_id, source))?;
                    let expected_key = process_log_key(process_id, entry.sequence)?;
                    if key != &expected_key {
                        return Err(corrupt_log(
                            process_id,
                            format!("log key {key:?} does not name sequence {}", entry.sequence),
                        ));
                    }
                    // Entries outside the head's window belong to a commit
                    // that landed after this head was read: the head/base view
                    // is still consistent without them, and the next load sees
                    // the newer head. Missing in-window entries are damage.
                    if entry.sequence < head.base_seq || entry.sequence >= head.next_seq {
                        continue;
                    }
                    stored_log.insert(entry.sequence, bytes.clone());
                    payloads.push(entry);
                }
                for sequence in head.base_seq..head.next_seq {
                    if !stored_log.contains_key(&sequence) {
                        return Err(corrupt_log(
                            process_id,
                            format!("log sequence {sequence} is missing"),
                        ));
                    }
                }
                payloads.sort_by_key(|entry| entry.sequence);
                let snapshot = ProcessLogCodec::assemble(base, &head, &payloads)
                    .map_err(|source| corrupt_process(process_id, source))?;
                (
                    snapshot,
                    Some(base_bytes),
                    stored_log,
                    head.epoch,
                    head.base_seq,
                    head.next_seq,
                    false,
                )
            }
            other => {
                return Err(corrupt_process(
                    process_id,
                    EnvelopeError::UnsupportedSchema(other).into(),
                ));
            }
        };
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
        original_base: stored_base,
        original_log: stored_log,
        epoch,
        base_seq,
        next_seq,
        legacy,
        original_recovery_index,
        original_outbox_index,
        original_ready_index,
    })
}

pub(crate) fn process_key(process_id: &str) -> Result<String, DocumentRepositoryError> {
    encoded_process_key(PROCESS_KEY_PREFIX, process_id)
}

pub(crate) fn process_base_key(process_id: &str) -> Result<String, DocumentRepositoryError> {
    Ok(format!(
        "{}/{}",
        process_key(process_id)?,
        PROCESS_BASE_SEGMENT
    ))
}

pub(crate) fn process_log_prefix(process_id: &str) -> Result<String, DocumentRepositoryError> {
    Ok(format!(
        "{}/{}/",
        process_key(process_id)?,
        PROCESS_LOG_SEGMENT
    ))
}

fn process_log_key(process_id: &str, sequence: u64) -> Result<String, DocumentRepositoryError> {
    Ok(format!(
        "{}{:020}",
        process_log_prefix(process_id)?,
        sequence
    ))
}

fn process_log_sequence(process_id: &str, key: &str) -> Option<u64> {
    let prefix = process_log_prefix(process_id).ok()?;
    let sequence: u64 = key.strip_prefix(&prefix)?.parse().ok()?;
    (process_log_key(process_id, sequence).ok().as_deref() == Some(key)).then_some(sequence)
}

/// Whether `key` is a process head (not the base or a log entry under it).
/// The census scans the aggregate prefix; only the head names a process.
pub(crate) fn is_process_head_key(key: &str) -> bool {
    key.strip_prefix(PROCESS_KEY_PREFIX)
        .is_some_and(|rest| !rest.is_empty() && !rest.contains('/'))
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

    use crate::scheduler::core::commit::aggregate::TaskSnapshot;
    use crate::scheduler::core::model::model_graph::NodeId;
    use crate::scheduler::core::model::operation::{
        ClaimToken, OperationId, OperationKind, OperationRecord, OperationState,
    };
    use crate::scheduler::core::model::task_graph::{TaskId, TaskState};
    use crate::scheduler::core::model::transition_plan::ProcessVersion;
    use crate::scheduler::core::outbox::records::{OutboxId, OutboxRecord};
    use crate::scheduler::durable::snapshot_codec::SnapshotCodecV3;
    use std::collections::HashMap;

    fn source_snapshot() -> ProcessSnapshot {
        let task = TaskId::new("step", 0).unwrap();
        let operation_id = OperationId {
            process_id: "process".into(),
            task: task.clone(),
            sequence: 0,
        };
        let pending_id = OperationId {
            process_id: "process".into(),
            task: task.clone(),
            sequence: 1,
        };
        let outbox_id = OutboxId {
            process_id: "process".into(),
            key: "out".into(),
        };
        ProcessSnapshot {
            process_id: "process".into(),
            version: ProcessVersion {
                graph: 1,
                process: 1,
                operations: 1,
            },
            tasks: HashMap::from([(
                task,
                TaskSnapshot {
                    model_node: NodeId::parse("model-step").unwrap(),
                    parent: None,
                    predecessors: vec![],
                    state: TaskState::Ready,
                    revision: 0,
                },
            )]),
            variables: BTreeMap::new(),
            operations: HashMap::from([
                (
                    operation_id.clone(),
                    OperationRecord {
                        id: operation_id,
                        kind: OperationKind::ExecuteTask,
                        state: OperationState::Claimed(ClaimToken {
                            owner: "owner".into(),
                            fence: 1,
                            lease_until_ms: 9_000,
                            heartbeat_revision: 0,
                        }),
                        revision: 1,
                    },
                ),
                (
                    pending_id.clone(),
                    OperationRecord::pending(pending_id, OperationKind::PropagateTask),
                ),
            ]),
            outbox: HashMap::from([(
                outbox_id.clone(),
                OutboxRecord::pending(outbox_id, "topic".into(), "{}".into(), 0).unwrap(),
            )]),
        }
    }

    fn loaded_aggregate(source: &ProcessSnapshot) -> LoadedProcess {
        let InstallWrite { write, .. } = build_create_transaction(source, 0, 800).unwrap();
        loaded_after_write(source, write)
    }

    fn head_document(
        source: &ProcessSnapshot,
        epoch: u64,
        base_seq: u64,
        next_seq: u64,
    ) -> Vec<u8> {
        ProcessLogCodec::encode_head(&HeadPayload {
            process_id: source.process_id.clone(),
            version: source.version.clone(),
            epoch,
            base_seq,
            next_seq,
        })
        .unwrap()
    }

    fn all_changes(source: &ProcessSnapshot) -> LogChanges {
        LogChanges {
            tasks: source.tasks.keys().cloned().collect(),
            variables: source.variables.keys().cloned().collect(),
            operations: source.operations.keys().cloned().collect(),
            outbox: source.outbox.keys().cloned().collect(),
        }
    }

    /// A transaction must carry at least one mutation, and no key may be empty
    /// or repeated, regardless of whether it is a guard or a mutation.
    #[test]
    fn transaction_validation_rejects_empty_batches_and_keys() {
        assert_eq!(
            DocumentTransaction::new().validate(),
            Err(DocumentStoreError(
                "document transaction has no mutations".into()
            ))
        );
        assert_eq!(
            DocumentTransaction::new().require_absent(""),
            Err(DocumentStoreError("empty document guard key".into()))
        );
        assert_eq!(
            DocumentTransaction::new().put("", b"value".to_vec()),
            Err(DocumentStoreError("empty document mutation key".into()))
        );
        assert_eq!(
            DocumentTransaction::new().delete(""),
            Err(DocumentStoreError("empty document mutation key".into()))
        );
    }

    /// Encoded keys refuse blank process ids, and decoding refuses foreign,
    /// odd-length, non-hex, uppercase-hex and non-UTF-8 keys.
    #[test]
    fn process_keys_are_validated_in_both_directions() {
        assert_eq!(
            process_key("   ").unwrap_err(),
            DocumentRepositoryError::EmptyProcessId
        );
        assert_eq!(
            outbox_index_key("").unwrap_err(),
            DocumentRepositoryError::EmptyProcessId
        );

        let foreign = decode_process_key("other/key").unwrap_err().to_string();
        assert!(foreign.contains("foreign key"));
        let odd = decode_process_key("kernel/aggregates/abc")
            .unwrap_err()
            .to_string();
        assert!(odd.contains("odd-length"));
        let non_hex = decode_process_key("kernel/aggregates/zz")
            .unwrap_err()
            .to_string();
        assert!(non_hex.contains("non-hex"));
        let uppercase = decode_process_key("kernel/aggregates/FF")
            .unwrap_err()
            .to_string();
        assert!(uppercase.contains("non-hex"));
        let non_utf8 = decode_process_key("kernel/aggregates/ff")
            .unwrap_err()
            .to_string();
        assert!(non_utf8.contains("non-utf8"));

        assert_eq!(decode_process_key(&process_key("é").unwrap()).unwrap(), "é");
        assert!(is_process_head_key(&process_key("p").unwrap()));
        assert!(!is_process_head_key(&process_base_key("p").unwrap()));
        assert!(!is_process_head_key(PROCESS_KEY_PREFIX));
    }

    /// A log key names its sequence only in canonical zero-padded form, and
    /// only under its own process's prefix.
    #[test]
    fn log_sequence_parsing_is_canonical() {
        let key = process_log_key("p", 5).unwrap();
        assert_eq!(process_log_sequence("p", &key), Some(5));
        assert_eq!(process_log_sequence("q", &key), None);
        let prefix = process_log_prefix("p").unwrap();
        assert_eq!(process_log_sequence("p", &format!("{prefix}abc")), None);
        assert_eq!(process_log_sequence("p", &format!("{prefix}5")), None);
    }

    #[test]
    fn replace_and_confirm_transactions_reject_a_foreign_snapshot() {
        let source = source_snapshot();
        let loaded = loaded_aggregate(&source);
        let mut foreign = source.clone();
        foreign.process_id = "foreign".into();

        let error = build_replace_transaction(
            &loaded,
            &foreign,
            &BTreeMap::new(),
            None,
            &LogChanges::default(),
        )
        .map(|_| ())
        .unwrap_err();
        assert_eq!(
            error,
            DocumentRepositoryError::ProcessIdentityMismatch {
                requested: "process".into(),
                stored: "foreign".into(),
            }
        );

        let error =
            build_outbox_confirm_transaction(&loaded, &foreign, std::iter::empty::<OutboxId>())
                .map(|_| ())
                .unwrap_err();
        assert_eq!(
            error,
            DocumentRepositoryError::ProcessIdentityMismatch {
                requested: "process".into(),
                stored: "foreign".into(),
            }
        );
    }

    /// A legacy or partially written aggregate is rebased: a missing base is
    /// guarded absent, an existing stale base is guarded exactly.
    #[test]
    fn replace_transaction_rebases_an_aggregate_without_a_base() {
        let source = source_snapshot();
        let base_key = process_base_key(&source.process_id).unwrap();
        let mut legacy = loaded_aggregate(&source);
        legacy.legacy = true;
        legacy.original_base = None;
        legacy.original_log.clear();

        let InstallWrite {
            transaction, write, ..
        } = build_replace_transaction(
            &legacy,
            &source,
            &BTreeMap::new(),
            None,
            &LogChanges::default(),
        )
        .unwrap();
        assert_eq!(write.base_seq, 0);
        assert_eq!(write.next_seq, 0);
        assert!(write.base.is_some());
        assert!(write.log_entry.is_none());
        assert!(
            transaction
                .guards()
                .iter()
                .any(|guard| { guard.key == base_key && guard.expected.is_none() })
        );

        let mut stale_base = legacy.clone();
        stale_base.original_base = Some(b"stale base".to_vec());
        let InstallWrite { transaction, .. } = build_replace_transaction(
            &stale_base,
            &source,
            &BTreeMap::new(),
            None,
            &LogChanges::default(),
        )
        .unwrap();
        assert!(transaction.guards().iter().any(|guard| {
            guard.key == base_key && guard.expected.as_deref() == Some(b"stale base".as_slice())
        }));
    }

    /// An L8 aggregate whose base document is missing but whose head says it
    /// is not legacy is the same rebase: the head is kept, a fresh base is
    /// guarded absent. This is the non-legacy half of the rebase decision.
    #[test]
    fn replace_transaction_rebases_a_partial_l8_aggregate() {
        let source = source_snapshot();
        let base_key = process_base_key(&source.process_id).unwrap();
        let mut partial = loaded_aggregate(&source);
        partial.legacy = false;
        partial.original_base = None;
        partial.original_log.clear();
        assert!(!partial.is_legacy());
        assert!(partial.original_base().is_none());

        let InstallWrite { transaction, write } = build_replace_transaction(
            &partial,
            &source,
            &BTreeMap::new(),
            None,
            &LogChanges::default(),
        )
        .unwrap();
        assert!(write.base.is_some());
        assert_eq!(write.base_seq, 0);
        assert_eq!(write.next_seq, 0);
        let guard = transaction
            .guards()
            .iter()
            .find(|guard| guard.key == base_key)
            .expect("the rebase must guard the base key");
        assert!(guard.expected.is_none());
    }

    /// The repair batch guards every observed document and rewrites only the
    /// indexes whose bytes differ; a consistent aggregate produces no batch.
    #[test]
    fn repair_transaction_rewrites_only_changed_indexes() {
        let source = source_snapshot();
        let loaded = loaded_aggregate(&source);
        let recovery_key = recovery_index_key("process").unwrap();
        let ready_key = ready_index_key("process").unwrap();
        let outbox_key = outbox_index_key("process").unwrap();

        let transaction = build_repair_transaction(&loaded, None, None)
            .unwrap()
            .expect("dropping both indexes is a change");
        assert!(
            transaction
                .guards()
                .iter()
                .any(|guard| { guard.key == outbox_key && guard.expected.is_some() })
        );
        assert!(transaction.mutations().contains(&DocumentMutation::Delete {
            key: recovery_key.clone()
        }));
        assert!(transaction.mutations().contains(&DocumentMutation::Delete {
            key: ready_key.clone()
        }));

        assert!(
            build_repair_transaction(
                &loaded,
                loaded.original_recovery_index(),
                loaded.original_ready_index(),
            )
            .unwrap()
            .is_none()
        );

        // Nothing observed means the guards are absent and the writes install
        // fresh documents.
        let empty = ProcessSnapshot::empty("empty").unwrap();
        let empty = loaded_aggregate(&empty);
        let transaction = build_repair_transaction(
            &empty,
            Some(b"recovery".as_slice()),
            Some(b"ready".as_slice()),
        )
        .unwrap()
        .expect("installing both indexes is a change");
        assert!(transaction.guards().iter().any(|guard| {
            guard.key == recovery_index_key("empty").unwrap() && guard.expected.is_none()
        }));
        assert!(transaction.mutations().contains(&DocumentMutation::Put {
            key: ready_index_key("empty").unwrap(),
            value: b"ready".to_vec(),
        }));
    }

    #[test]
    fn reclaim_transactions_delete_observed_entries_and_logs() {
        let source = ProcessSnapshot::empty("gone").unwrap();
        let loaded = loaded_aggregate(&source);
        let entry_key = format!("{}entry", recovery_entry_process_prefix("gone").unwrap());
        let transaction =
            build_reclaim_transaction(&loaded, &[(entry_key.clone(), b"entry".to_vec())]).unwrap();
        assert!(transaction.guards().iter().any(|guard| {
            guard.key == entry_key && guard.expected.as_deref() == Some(b"entry".as_slice())
        }));
        assert!(
            transaction
                .mutations()
                .contains(&DocumentMutation::Delete { key: entry_key })
        );

        let base_key = process_base_key("gone").unwrap();
        let log_key = process_log_key("gone", 3).unwrap();
        let orphan = build_orphan_reclaim_transaction(
            "gone",
            Some(b"base".to_vec()),
            &[(log_key.clone(), b"log".to_vec())],
            Some(b"recovery".to_vec()),
            None,
            Some(b"ready".to_vec()),
            &[(String::from("entry"), b"value".to_vec())],
        )
        .unwrap();
        for key in [
            process_key("gone").unwrap(),
            base_key.clone(),
            log_key.clone(),
            recovery_index_key("gone").unwrap(),
            outbox_index_key("gone").unwrap(),
            ready_index_key("gone").unwrap(),
            "entry".into(),
        ] {
            assert!(
                orphan
                    .mutations()
                    .contains(&DocumentMutation::Delete { key: key.clone() }),
                "{key:?} must be deleted"
            );
        }
        assert!(orphan.guards().iter().any(|guard| {
            guard.key == base_key && guard.expected.as_deref() == Some(b"base".as_slice())
        }));
        assert!(orphan.guards().iter().any(|guard| {
            guard.key == log_key && guard.expected.as_deref() == Some(b"log".as_slice())
        }));
        let head_guard = orphan
            .guards()
            .iter()
            .find(|guard| guard.key == process_key("gone").unwrap())
            .expect("the orphan head must be guarded absent");
        assert!(head_guard.expected.is_none());
    }

    /// Migration is only meaningful for a legacy aggregate, and it replaces a
    /// stale base and deletes stale log entries the legacy head cannot name.
    #[test]
    fn migration_transaction_replaces_stale_family_documents() {
        let error = build_migration_transaction(&loaded_aggregate(&source_snapshot()))
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(error.contains("already in the L8 format"));

        let source = source_snapshot();
        let log_key = process_log_key("process", 0).unwrap();
        let legacy = LoadedProcess {
            snapshot: source.clone(),
            original_document: ProcessDocumentCodec::encode(&source).unwrap(),
            original_base: Some(b"stale base".to_vec()),
            original_log: BTreeMap::from([(0, b"stale log".to_vec())]),
            epoch: 0,
            base_seq: 0,
            next_seq: 0,
            legacy: true,
            original_recovery_index: None,
            original_outbox_index: None,
            original_ready_index: None,
        };
        let (transaction, head, base) = build_migration_transaction(&legacy).unwrap();
        assert!(transaction.guards().iter().any(|guard| {
            guard.key == log_key && guard.expected.as_deref() == Some(b"stale log".as_slice())
        }));
        assert!(
            transaction
                .mutations()
                .contains(&DocumentMutation::Delete { key: log_key })
        );
        assert!(transaction.mutations().contains(&DocumentMutation::Put {
            key: process_key("process").unwrap(),
            value: head,
        }));
        assert!(transaction.mutations().contains(&DocumentMutation::Put {
            key: process_base_key("process").unwrap(),
            value: base,
        }));
    }

    /// Every inconsistency the loader can detect is classified, and a valid
    /// family loads.
    #[test]
    fn decode_loaded_process_classifies_each_inconsistency() {
        let source = source_snapshot();
        let log_key = process_log_key("process", 0).unwrap();
        let base = ProcessLogCodec::encode_base(&source, 0).unwrap();
        let log = ProcessLogCodec::encode_log(&source, 0, &all_changes(&source)).unwrap();
        let recovery = recovery_index_document(&source).unwrap();
        let outbox = outbox_index_document(&source).unwrap();
        let head = head_document(&source, 1, 0, 1);

        let loaded = decode_loaded_process(
            "process",
            head.clone(),
            Some(base.clone()),
            &[(log_key.clone(), log.clone())],
            recovery.clone(),
            outbox.clone(),
            None,
        )
        .unwrap();
        assert_eq!(loaded.snapshot, source);
        assert!(!loaded.is_legacy());
        assert_eq!(loaded.original_log().len(), 1);

        // The head must name the process it is filed under.
        let error = decode_loaded_process(
            "other",
            head.clone(),
            Some(base.clone()),
            &[],
            None,
            None,
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("head names"));

        // An L8 head without a base is damage.
        let error = decode_loaded_process("process", head.clone(), None, &[], None, None, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("no base snapshot"));

        // A legacy-schema base is not this family.
        let legacy_base = ProcessEnvelope::seal(SnapshotCodecV3::encode(&source).unwrap()).unwrap();
        let error = decode_loaded_process(
            "process",
            head.clone(),
            Some(legacy_base),
            &[],
            None,
            None,
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("base uses schema"));

        // The base must cover the sequence the head claims.
        let moved_base = ProcessLogCodec::encode_base(&source, 7).unwrap();
        let error = decode_loaded_process(
            "process",
            head.clone(),
            Some(moved_base),
            &[],
            None,
            None,
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("base covers sequence 7"));

        // A log entry filed under the wrong key, or from another schema, is
        // refused.
        let wrong_key = process_log_key("process", 1).unwrap();
        let error = decode_loaded_process(
            "process",
            head.clone(),
            Some(base.clone()),
            &[(wrong_key, log.clone())],
            recovery.clone(),
            outbox.clone(),
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("does not name sequence"));
        let legacy_log = ProcessEnvelope::seal(SnapshotCodecV3::encode(&source).unwrap()).unwrap();
        let error = decode_loaded_process(
            "process",
            head.clone(),
            Some(base.clone()),
            &[(log_key.clone(), legacy_log)],
            recovery.clone(),
            outbox.clone(),
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("foreign schema"));

        // An in-window sequence with no entry is damage; an out-of-window
        // entry is a newer commit and is simply not folded in.
        let error = decode_loaded_process(
            "process",
            head.clone(),
            Some(base.clone()),
            &[],
            recovery.clone(),
            outbox.clone(),
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("log sequence 0 is missing"));
        let outside_key = process_log_key("process", 5).unwrap();
        let outside = ProcessLogCodec::encode_log(&source, 5, &all_changes(&source)).unwrap();
        let loaded = decode_loaded_process(
            "process",
            head.clone(),
            Some(base.clone()),
            &[(log_key.clone(), log.clone()), (outside_key, outside)],
            recovery.clone(),
            outbox.clone(),
            None,
        )
        .unwrap();
        assert_eq!(loaded.original_log().len(), 1);

        // An entry from below the window (a base already covers it) is stale
        // too: only in-window sequences survive the fold.
        let stale_key = process_log_key("process", 0).unwrap();
        let stale_log = ProcessLogCodec::encode_log(&source, 0, &all_changes(&source)).unwrap();
        let rebased = ProcessLogCodec::encode_base(&source, 2).unwrap();
        let in_window = ProcessLogCodec::encode_log(&source, 2, &all_changes(&source)).unwrap();
        let head_at_two = head_document(&source, 1, 2, 3);
        let loaded = decode_loaded_process(
            "process",
            head_at_two,
            Some(rebased),
            &[
                (stale_key, stale_log),
                (process_log_key("process", 2).unwrap(), in_window),
            ],
            recovery.clone(),
            outbox.clone(),
            None,
        )
        .unwrap();
        assert_eq!(loaded.original_log().len(), 1);
        assert_eq!(loaded.base_seq(), 2);

        // The decoded snapshot must be the requested process.
        let legacy = ProcessDocumentCodec::encode(&source).unwrap();
        let error = decode_loaded_process("other", legacy.clone(), None, &[], None, None, None)
            .unwrap_err();
        assert_eq!(
            error,
            DocumentRepositoryError::ProcessIdentityMismatch {
                requested: "other".into(),
                stored: "process".into(),
            }
        );

        // A legacy head keeps stale log documents so migration/reclaim can
        // delete them, and the derived indexes must match the rebuilt snapshot.
        let legacy_loaded = decode_loaded_process(
            "process",
            legacy.clone(),
            Some(b"stale base".to_vec()),
            &[(log_key.clone(), log.clone())],
            recovery.clone(),
            outbox.clone(),
            None,
        )
        .unwrap();
        assert!(legacy_loaded.is_legacy());
        assert_eq!(legacy_loaded.original_log().len(), 1);
        assert_eq!(
            legacy_loaded.original_base(),
            Some(b"stale base".as_slice())
        );

        assert!(matches!(
            decode_loaded_process(
                "process",
                legacy.clone(),
                None,
                &[],
                Some(b"wrong recovery".to_vec()),
                outbox.clone(),
                None
            ),
            Err(DocumentRepositoryError::RecoveryIndexMismatch { .. })
        ));

        // A pre-A2 recovery projection (every non-terminal operation) still
        // loads: it differs from the current projection but matches the legacy
        // one, so the second half of the mismatch detection is what saves it.
        let legacy_recovery = legacy_recovery_index_document(&source).unwrap();
        assert_ne!(legacy_recovery, recovery);
        let accepted = decode_loaded_process(
            "process",
            legacy.clone(),
            None,
            &[],
            legacy_recovery,
            outbox.clone(),
            None,
        )
        .unwrap();
        assert!(accepted.is_legacy());

        assert!(matches!(
            decode_loaded_process(
                "process",
                legacy,
                None,
                &[],
                recovery,
                Some(b"wrong outbox".to_vec()),
                None
            ),
            Err(DocumentRepositoryError::OutboxIndexMismatch { .. })
        ));
    }

    #[test]
    fn repository_and_store_errors_are_converted_and_displayed() {
        let codec: DocumentRepositoryError =
            ProcessDocumentError::Snapshot(SnapshotCodecError::InvalidMagic).into();
        assert!(matches!(&codec, DocumentRepositoryError::Codec(_)));
        assert!(codec.to_string().contains("document repository error"));

        let store: DocumentRepositoryError = DocumentStoreError("boom".into()).into();
        assert_eq!(
            store,
            DocumentRepositoryError::Store(DocumentStoreError("boom".into()))
        );
        assert_eq!(DocumentStoreError("boom".into()).to_string(), "boom");
    }

    #[test]
    fn document_transaction_rejects_ambiguity_during_construction() {
        let duplicate_guard = DocumentTransaction::new()
            .require_absent("aggregate")
            .unwrap()
            .require_exact("aggregate", b"old".to_vec())
            .unwrap_err()
            .to_string();
        assert!(duplicate_guard.contains("duplicate document guard"));

        let duplicate_mutation = DocumentTransaction::new()
            .put("index", b"value".to_vec())
            .unwrap()
            .delete("index")
            .unwrap_err()
            .to_string();
        assert!(duplicate_mutation.contains("duplicate document mutation"));
    }
}
