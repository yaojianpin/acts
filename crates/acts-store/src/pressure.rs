//! The database half of the engine's write backpressure.
//!
//! The engine's store-writer shards bound their own queues and report
//! saturation to their producers. A backend can still become the slower side:
//! RocksDB has its own memtable/WAL pipeline, and a sharded store writes to
//! several physical databases whose throughput differs. [`WriteGate`] is the
//! small primitive both use to bound writes at the database boundary:
//!
//! - a write takes one permit before it enters the backend, so at most
//!   `capacity` writes are in flight per physical database;
//! - while every permit is held (or a writer is queued for one), the gate
//!   reports saturated, which [`KvStore::saturated`](acts::KvStore::saturated)
//!   exposes to the engine's writer.
//!
//! Waiting is deliberate: engine writes that are already admitted have to be
//! applied, so they queue here rather than fail. The upstream store-writer
//! backlog fills while they wait, which is what stops *new* work — exactly the
//! same two-stage behavior the writer already has between its producers and
//! its shards.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use acts::{ActError, Result};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// A bounded write pipeline for one physical database.
#[derive(Debug)]
pub(crate) struct WriteGate {
    permits: Arc<Semaphore>,
    waiters: AtomicUsize,
}

/// One admitted write. The permit returns to the gate when this is dropped.
pub(crate) struct WritePermit {
    _permit: OwnedSemaphorePermit,
}

impl WriteGate {
    /// A gate that admits `capacity` concurrent writes (at least one).
    pub(crate) fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            permits: Arc::new(Semaphore::new(capacity.max(1))),
            waiters: AtomicUsize::new(0),
        })
    }

    /// Wait for one write slot and hold it until the returned permit drops.
    pub(crate) async fn acquire(self: &Arc<Self>) -> Result<WritePermit> {
        if let Ok(permit) = self.permits.clone().try_acquire_owned() {
            return Ok(WritePermit { _permit: permit });
        }
        self.waiters.fetch_add(1, Ordering::AcqRel);
        let permit = self.permits.clone().acquire_owned().await;
        self.waiters.fetch_sub(1, Ordering::AcqRel);
        permit
            .map(|permit| WritePermit { _permit: permit })
            .map_err(|_| ActError::Store("store write gate is closed".to_string()))
    }

    /// Whether a new write would have to wait for a slot. A gate is saturated
    /// while every slot is held, and while at least one writer is queued — the
    /// two states in which admitting more work only builds a second queue in
    /// front of this one.
    pub(crate) fn saturated(&self) -> bool {
        self.waiters.load(Ordering::Acquire) > 0 || self.permits.available_permits() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn a_full_gate_reports_saturation_until_a_permit_returns() {
        let gate = WriteGate::new(1);
        assert!(!gate.saturated());
        let permit = gate.acquire().await.unwrap();
        assert!(gate.saturated(), "the only slot is held");
        drop(permit);
        assert!(!gate.saturated(), "a released slot re-opens the gate");
    }

    #[tokio::test]
    async fn waiting_writers_keep_the_gate_saturated() {
        let gate = WriteGate::new(1);
        let held = gate.acquire().await.unwrap();
        let clone = gate.clone();
        let waiter = tokio::spawn(async move { clone.acquire().await.map(|_| ()) });
        // Let the waiter queue; the gate must report saturation while it does.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(gate.saturated());
        drop(held);
        waiter.await.unwrap().unwrap();
        assert!(!gate.saturated());
    }

    #[tokio::test]
    async fn a_zero_capacity_gate_admits_one_write() {
        let gate = WriteGate::new(0);
        assert!(!gate.saturated());
        let _permit = gate.acquire().await.unwrap();
        assert!(gate.saturated());
    }
}
