mod collection;
pub mod data;
mod lease;
mod memory;
pub mod query;

#[allow(clippy::module_inception)]
mod store;

use data::*;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value as JsonValue;
#[allow(unused_imports)]
pub use store::Store;

use crate::{ActError, Result};
use query::*;
use std::error::Error;
use strum::{AsRefStr, EnumIter};

#[allow(unused_imports)]
pub use memory::MemoryStore;

pub use lease::{
    DEFAULT_LEASE_RENEW, DEFAULT_LEASE_TTL, DbLease, FencedStore, LEASE_KEY, LeaseKeeper,
    LeaseRecord,
};

fn map_db_err(err: impl Error) -> ActError {
    ActError::Store(err.to_string())
}

#[derive(Debug, Clone, AsRefStr, PartialEq, Hash, Eq, EnumIter)]
pub enum StoreIden {
    #[strum(serialize = "packages")]
    Packages,
    #[strum(serialize = "models")]
    Models,
    #[strum(serialize = "procs")]
    Procs,
    #[strum(serialize = "tasks")]
    Tasks,
    #[strum(serialize = "vars")]
    Vars,
    #[strum(serialize = "messages")]
    Messages,
    #[strum(serialize = "deliveries")]
    Deliveries,
    #[strum(serialize = "events")]
    Events,
    #[strum(serialize = "ops")]
    Ops,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PageData<T> {
    pub count: usize,
    pub page_num: usize,
    pub page_count: usize,
    pub page_size: usize,
    pub rows: Vec<T>,
}

pub trait DbCollectionIden {
    /// The collection's key prefix — the namespace its documents and index
    /// rows live under. It is a plain string so a crate outside this one can
    /// define a collection of its own (the ACL's users and sessions do): the
    /// store keeps no registry of names, so two collections must choose
    /// different prefixes to stay apart.
    fn iden() -> String;
    fn indexed_fields() -> &'static [&'static str] {
        &[]
    }

    /// Indexed fields whose index-key order is also the query `order_by`
    /// order. A field must only be listed when every stored value has the
    /// same JSON type and that type is encoded in order-preserving form
    /// (currently the fixed-width unsigned/positive integer encoding).
    fn ordered_index_fields() -> &'static [&'static str] {
        &[]
    }
    fn version() -> i32 {
        0
    }

    /// Default deserialization for the current version — can be called by
    /// overriding `upcast` impls when the version matches [`Self::version()`].
    fn upcast_current(mut value: JsonValue) -> Result<Self>
    where
        Self: DeserializeOwned,
    {
        // Ensure v field exists for backward compatibility with older records
        if let JsonValue::Object(ref mut map) = value {
            map.entry("v".to_string())
                .or_insert_with(|| JsonValue::Number(serde_json::Number::from(0)));
        }
        serde_json::from_value(value).map_err(map_db_err)
    }

    fn upcast(value: JsonValue) -> Result<Self>
    where
        Self: DeserializeOwned,
    {
        Self::upcast_current(value)
    }
}

pub struct ScanOptions {
    /// list is in reverse order
    pub is_rev: bool,
    /// scan operation
    pub op: ScanOperation,
    /// The prefix that bounds the scan. All returned keys must start with this prefix.
    /// For point ops (Eq/Ne/In) this is the field-level prefix
    /// (e.g., "tasks-state-"), and `key` is the full value prefix passed to
    /// [`KvStore::scan_prefix`].
    /// For [`ScanOperation::Range`] the prefix is the same field-level prefix;
    /// the interval bounds are full keys and already contain the prefix.
    pub prefix: String,
}

impl ScanOptions {
    pub fn new(op: ScanOperation, prefix: String, is_rev: bool) -> Self {
        Self { is_rev, op, prefix }
    }
}

pub enum ScanOperation {
    /// Not equal — keys that start with the parent prefix but NOT with `key`
    Ne,

    /// Equal — keys that start with `key` (a full value-key prefix)
    Eq,

    /// Key starts with any one of the given full value-key prefixes (e.g.,
    /// "tasks-state-Completed-").
    In { values: Vec<String> },

    /// Half-open interval over full keys inside the scan prefix: every
    /// returned key satisfies `lower <= key` (when `lower` is `Some`) and
    /// `key < upper` (when `upper` is `Some`).
    ///
    /// Bounds are computed by the collection layer, never by backends:
    /// inclusive value-range ends are encoded with the sentinel
    /// [`crate::utils::consts::KEY_SEP_SUCC`], so a bound `..<v>-` group is
    /// addressed as `upper = ..<v>+SUCC` (covers every `..<v>-<id>` key and
    /// nothing above it). Backends only compare full key strings.
    Range {
        lower: Option<String>,
        upper: Option<String>,
    },
}

/// One mutation of an atomic [`KvStore::batch`] write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreBatchOp {
    /// Store (overwrite) `value` under `key`.
    Put { key: String, value: Vec<u8> },
    /// Remove `key`.
    Delete { key: String },
}

/// The state a [`KvStore::batch`] requires `key` to hold *at the moment its ops
/// commit* — the read a read-modify-write decided on, moved inside the write's
/// atomic unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreGuard {
    /// The key whose stored bytes gate the batch.
    pub key: String,
    /// The exact bytes `key` MUST hold for the batch to apply; `None` requires
    /// the key to be absent.
    pub expected: Option<Vec<u8>>,
}

impl StoreGuard {
    /// Require `key` to still hold `expected`.
    pub fn holds(key: impl Into<String>, expected: impl Into<Vec<u8>>) -> Self {
        Self {
            key: key.into(),
            expected: Some(expected.into()),
        }
    }

    /// Require `key` to still be absent.
    pub fn absent(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            expected: None,
        }
    }

    /// Whether `stored` satisfies this guard.
    pub fn matches(&self, stored: Option<&[u8]>) -> bool {
        match (&self.expected, stored) {
            (None, None) => true,
            (Some(expected), Some(stored)) => expected.as_slice() == stored,
            _ => false,
        }
    }
}

#[async_trait::async_trait]
pub trait KvStore: Send + Sync {
    /// Read one key. Backends that support a native batch read should override
    /// [`KvStore::many`] for efficiency; the default loops over `one`.
    async fn one(&self, key: &str) -> Result<Option<Vec<u8>>>;

    /// Read several keys in one logical call. The default preserves
    /// compatibility for stores without a native batch read; backends with a
    /// network or SQL round trip should override it.
    async fn many(&self, keys: &[String]) -> Result<Vec<Option<Vec<u8>>>> {
        let mut values = Vec::with_capacity(keys.len());
        for key in keys {
            values.push(self.one(key).await?);
        }
        Ok(values)
    }

    /// Write one key.
    async fn put(&self, key: &str, value: Vec<u8>) -> Result<()>;

    /// Delete one key.
    async fn delete(&self, key: &str) -> Result<()>;

    /// Apply `ops` as one unit, and only while every guard still holds.
    ///
    /// One call, one atomic step of the backend:
    ///
    /// - with no guards, all of `ops` become visible together, or — on a
    ///   backend that commits atomically — none of them does. The collection
    ///   layer uses this for `create`/`update`/`delete`, so a document row and
    ///   its index entries are written together instead of as a sequence of
    ///   independently committed keys a mid-write failure could tear.
    /// - with guards, the guard reads join that same unit: `Ok(false)` says a
    ///   guard did not hold **and no op was applied**, which is the caller's
    ///   signal to re-read and decide again, never a partial write. This is
    ///   what makes a read-modify-write safe against another writer — the
    ///   read the write was computed from is re-checked at commit time, not
    ///   before it — and it is the primitive [`crate::DbLease`] builds an
    ///   exclusive database lease on.
    ///
    /// A backend that cannot put the guard reads in the same atomic unit as
    /// the writes MUST report an error rather than commit a check-then-write:
    /// the default below does exactly that, so a custom [`KvStore`] inherits a
    /// safe refusal instead of a silent race. `MemoryStore` (in `acts`) and
    /// the `acts-store` backends `SledStore`, `SqliteStore` and
    /// `PostgresStore` commit through a native transaction with the guards
    /// included (a guardless single-op batch falls back to `put`/`delete`,
    /// skipping transaction overhead).
    ///
    /// A guard set rather than one optional guard, because a decorator has to
    /// be able to add its own condition to the caller's: [`crate::FencedStore`]
    /// commits the lease row *and* whatever guard the caller passed, in one
    /// batch. Every caller in this workspace passes `&[]` or a one-element
    /// slice — a read-modify-write over several documents (a model and its
    /// trigger rows, a process and all of its rows) is what the shape is for
    /// beyond that.
    async fn batch(&self, ops: &[StoreBatchOp], guards: &[StoreGuard]) -> Result<bool> {
        if guards.is_empty() {
            for op in ops {
                match op {
                    StoreBatchOp::Put { key, value } => self.put(key, value.clone()).await?,
                    StoreBatchOp::Delete { key } => self.delete(key).await?,
                }
            }
            return Ok(true);
        }
        let _ = ops;
        Err(ActError::Store(
            "this store cannot commit a guarded batch: it cannot make a read \
             and the writes it decided on one atomic unit, so an exclusive \
             database lease cannot be provided on it"
                .to_string(),
        ))
    }

    /// Return every entry whose key starts with `key` and matches `options`.
    ///
    /// Entry order is unspecified: a backend may iterate an ordered key space
    /// (SQLite/Postgres `ORDER BY key`, an in-memory `BTreeMap`). Callers that
    /// need an order MUST impose it themselves.
    async fn scan_prefix(&self, key: &str, options: ScanOptions) -> Result<Vec<(String, Vec<u8>)>>;
}

/// One collection of documents, each with an id and the index rows derived
/// from its indexed fields.
///
/// The three mutations are read-modify-writes of a document (they read the
/// stored row to compute which index rows to drop) and each applies its data
/// row and index rows as one atomic [`KvStore::batch`]. They are serialized
/// per document, so any combination of `create`/`update`/`delete` racing on
/// one id leaves the index rows describing the document that ends up stored —
/// never a query result the data row does not back.
///
/// That serialization is per document *and per process*: `batch` makes a
/// single write all-or-nothing, it does not make a read-then-batch pair atomic
/// against another writer. A database written by more than one process is
/// coordinated outside this layer — [`crate::DbLease`] takes an exclusive
/// lease on the database and commits every write through the fence
/// ([`KvStore::batch`] under a [`StoreGuard`]), so an instance that lost the
/// lease has its writes refused instead of interleaving with the holder's. See
/// the document-lock registry notes in the store's `collection` module.
///
/// [`DbCollection::query`] answers one page; recovery, cleanup and
/// reconciliation read whole match sets through
/// [`DbCollection::query_all`]/[`DbCollection::matching_ids`]/
/// [`DbCollection::delete_all`], which no page size can silently truncate.
#[async_trait::async_trait]
pub trait DbCollection: Send + Sync {
    type Item;
    async fn exists(&self, id: &str) -> Result<bool>;
    async fn find(&self, id: &str) -> Result<Self::Item>;
    /// Like [`DbCollection::find`], but a missing row is `Ok(None)` instead of
    /// an error. Only the explicit not-found case is normalized — a backend
    /// failure is still an error, so callers can tell "no record" from "the
    /// store is unavailable".
    async fn find_opt(&self, id: &str) -> Result<Option<Self::Item>>;
    /// One page of the matching documents — at most `query.limit` rows;
    /// `count` reports how many rows match in total.
    async fn query(&self, query: &Query) -> Result<PageData<Self::Item>>;
    /// Every row matching `query.filter`, in `query.order_by` order (id order
    /// when it is unset) — the exhaustive counterpart of
    /// [`DbCollection::query`], whose rows stop at `query.limit`.
    ///
    /// `query.limit` sizes one document read batch, never the result;
    /// `query.offset` is not applied (an exhaustive read has no page to skip
    /// to).
    async fn query_all(&self, query: &Query) -> Result<Vec<Self::Item>>;
    /// Every id matching `filter` (`None` = every id of the collection),
    /// ascending — a delete of the whole match set reads its ids through
    /// here, never through a limited [`DbCollection::query`].
    async fn matching_ids(&self, filter: Option<&Filter>) -> Result<Vec<String>>;
    /// Delete every row matching `filter` (`None` = every row of the
    /// collection). Each row commits on its own — its index rows and its data
    /// row as one batch — so a failure mid-way leaves the remaining rows to
    /// the next pass rather than a row without its indexes.
    async fn delete_all(&self, filter: Option<&Filter>) -> Result<()> {
        for id in self.matching_ids(filter).await? {
            self.delete(&id).await?;
        }
        Ok(())
    }
    /// The first row matching `query.filter` for which `pred` holds, reading
    /// row bodies in `query.limit`-sized batches and stopping at the first
    /// hit — an existence test over a match set that may exceed one page.
    ///
    /// [`DbCollection::query`] could miss the row beyond its page, and
    /// [`DbCollection::query_all`] would materialize every row of the set;
    /// `query.limit` bounds the rows read at once here, not the search.
    async fn find_matching(
        &self,
        query: &Query,
        pred: &(dyn for<'a> Fn(&'a Self::Item) -> bool + Sync),
    ) -> Result<Option<Self::Item>>;
    /// Write `data` as the document of its id, replacing any stored document
    /// together with the index rows it no longer matches.
    async fn create(&self, data: &Self::Item) -> Result<bool>;
    /// Same write as [`DbCollection::create`].
    async fn update(&self, data: &Self::Item) -> Result<bool>;
    /// Remove the document and every index row of it.
    async fn delete(&self, id: &str) -> Result<bool>;
}
