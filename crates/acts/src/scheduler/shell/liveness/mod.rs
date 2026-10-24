//! Liveness: the in-memory wake plumbing between the durable facts and the
//! driver's loops.
//!
//! Every piece here is an accelerator, never a source of truth. The durable
//! ready index is what makes work official; a lost wake only costs latency,
//! because the owning pass re-derives the same due set from the store.

use super::*;

/// Best-effort wake delivery into the host's queue. Losing a wake is safe:
/// the operation also owns a durable ready record, and the driver's due pass
/// picks it up. A failed send is counted and wakes the due-pass notification.
#[derive(Clone)]
pub(crate) struct QueueWake {
    tx: Sender<OperationId>,
}

impl QueueWake {
    pub(super) fn wake(&self, operation: &OperationId) -> bool {
        self.tx.try_send(operation.clone()).is_ok()
    }
}

impl KernelHost {
    /// Wake one operation. A wake is an optimization over the durable ready
    /// index: a full queue drops it, the ready record remains, and the due pass
    /// is notified to pick it up before the next tick.
    pub(super) fn wake(&self, operation: &OperationId) -> bool {
        let sent = QueueWake {
            tx: self.wake_tx.clone(),
        }
        .wake(operation);
        if !sent {
            self.ready_metrics.lock().wake_failures += 1;
            self.ready.notify_one();
        }
        sent
    }

    /// The wakeup a durable run handle registers before re-reading the
    /// aggregate (see [`crate::RunHandle::wait`]).
    ///
    /// The entry is per pid and taken by [`Self::notify_settled`], so a waiter
    /// that arrives after the settle gets a fresh entry and sees the terminal
    /// aggregate on its own re-read: the notification only ever shortens the
    /// wait, it never carries the outcome.
    pub(crate) fn run_waiter(&self, process_id: &str) -> Arc<tokio::sync::Notify> {
        self.settled
            .lock()
            .entry(process_id.to_string())
            .or_default()
            .clone()
    }

    /// Wake every handle waiting on `process_id`: the run's commit settled it
    /// (its aggregate is durable and readable) or its durable record was
    /// reclaimed. Taking the entry keeps the map bounded by pids actually
    /// being waited on.
    pub(in crate::scheduler::shell) fn notify_settled(&self, process_id: &str) {
        let waiter = self.settled.lock().remove(process_id);
        if let Some(waiter) = waiter {
            waiter.notify_waiters();
        }
    }
}
