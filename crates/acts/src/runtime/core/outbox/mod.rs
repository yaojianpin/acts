//! The durable outbox domain: records, indexes, dispatch and the worker loop.

pub(crate) mod outbox_codec;
pub(crate) mod outbox_dispatch;
pub(crate) mod outbox_index;
pub(crate) mod outbox_repository;
pub(crate) mod records;
