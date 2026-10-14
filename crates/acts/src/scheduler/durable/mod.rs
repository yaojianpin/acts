//! Byte-level persistence: aggregate document stores, codecs and the storage adapters.
//!
//! The only layer allowed to reach the key-value store.

pub(crate) mod async_document_store;
pub(crate) mod document;
pub(crate) mod document_store;
pub(crate) mod envelope;
pub(crate) mod snapshot_codec;
