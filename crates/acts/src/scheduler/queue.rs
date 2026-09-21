use crate::ActError;
use crate::event::ProcessGate;
use crate::{
    Result,
    scheduler::{Process, Task},
};
use parking_lot::Mutex;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::sync::mpsc;

/// The scheduler's in-memory backlog: a fixed pool of bounded per-lane job
/// queues.
///
/// A job is routed to its lane by the pid hash shared with the event gate
/// ([`ProcessGate::lane_index`]), so every job of one process stays FIFO on one
/// lane while independent processes overlap on different lanes. Each lane is
/// bounded, and a full lane reports [`ActError::QueueFull`] to its producer,
/// which turns that into a durable outbox record. A slow consumer therefore
/// cannot absorb unbounded work in memory: the resident backlog of
/// `Arc<Task>`/`Arc<Process>` items is bounded by `lanes() × lane_capacity()`
/// (`scheduler_queue_cap` split across the lanes).
#[derive(Debug, Clone)]
pub struct Queue {
    /// One bounded channel per lane, indexed by [`ProcessGate::lane_index`].
    senders: Arc<Vec<mpsc::Sender<QueueData>>>,
    /// Lane receivers, handed to the workers once by [`Self::take_receivers`].
    receivers: Arc<Mutex<Option<Vec<mpsc::Receiver<QueueData>>>>>,
    gate: ProcessGate,
    alive: Arc<AtomicBool>,
    high_watermark: Arc<AtomicUsize>,
    capacity: usize,
    lane_capacity: usize,
}

#[derive(Debug)]
pub enum QueueData {
    Task {
        task: Arc<Task>,
        /// Execution-time lease: a queued task must remain executable even if
        /// its finished process is evicted before its lane reaches this item.
        proc: Arc<Process>,
    },
    Next {
        task: Arc<Task>,
        proc: Arc<Process>,
    },
    /// One-hop unhandled-error propagation.
    Error {
        task: Arc<Task>,
        proc: Arc<Process>,
    },
    /// One-hop abort propagation.
    AbortPropagation {
        task: Arc<Task>,
        proc: Arc<Process>,
    },
    /// Wake sentinel for an idle lane worker (see [`Queue::abort`]); it belongs
    /// to no process, so it is pushed to a lane directly.
    Abort,
}

impl Queue {
    pub(crate) fn new(capacity: usize, gate: ProcessGate) -> Arc<Self> {
        let capacity = capacity.max(1);
        let lanes = gate.lanes();
        // The cap is the *total* resident backlog, split evenly across the
        // lanes, so the memory bound does not grow with the worker count. A cap
        // below the lane count cannot be split without starving a lane — every
        // lane keeps one slot, so the effective bound is then the lane count
        // itself.
        let lane_capacity = (capacity / lanes).max(1);
        let mut senders = Vec::with_capacity(lanes);
        let mut receivers = Vec::with_capacity(lanes);
        for _ in 0..lanes {
            let (sender, receiver) = mpsc::channel::<QueueData>(lane_capacity);
            senders.push(sender);
            receivers.push(receiver);
        }

        Arc::new(Self {
            senders: Arc::new(senders),
            receivers: Arc::new(Mutex::new(Some(receivers))),
            gate,
            alive: Arc::new(AtomicBool::new(true)),
            high_watermark: Arc::new(AtomicUsize::new(0)),
            capacity,
            lane_capacity,
        })
    }

    /// Number of lanes (the worker pool size). Lanes are addressed by
    /// [`ProcessGate::lane_index`], which is derived from this same gate.
    pub fn lanes(&self) -> usize {
        self.senders.len()
    }

    /// The configured bound on the resident backlog. The effective bound is
    /// `lanes() × lane_capacity()`, which equals this number whenever the cap
    /// covers every lane (and is the lane count when it does not).
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Per-lane bound: the cap divided by the lane count (at least one). This is
    /// how much work one process can have buffered in memory at once —
    /// everything beyond it is durably queued instead.
    pub fn lane_capacity(&self) -> usize {
        self.lane_capacity
    }

    /// Current number of items buffered in the lanes (not including items a
    /// worker already took for execution). Read from the lanes themselves, so it
    /// is exact and can never drift past the effective bound.
    pub fn depth(&self) -> usize {
        self.buffered()
    }

    pub fn high_watermark(&self) -> usize {
        self.high_watermark.load(Ordering::Acquire)
    }

    /// Pin the queue's consumers as running. If they die — even by an
    /// unexpected unwind — the lease makes later `send`s fail instead of
    /// silently accepting work that will never run.
    pub(crate) fn consumer_lease(&self) -> QueueConsumerLease {
        QueueConsumerLease {
            alive: self.alive.clone(),
        }
    }

    /// Hand the lane receivers to the worker pool. The event loop calls this
    /// once; a later call yields nothing, so a lane never has two consumers.
    pub(crate) fn take_receivers(&self) -> Vec<mpsc::Receiver<QueueData>> {
        self.receivers.lock().take().unwrap_or_default()
    }

    /// Queue a task for execution. Fails when the queue consumer is gone or the
    /// task's process was deallocated: a refused dispatch must be visible to
    /// the caller, which may hold a durable outbox record for the task.
    pub(crate) fn send(&self, task: &Arc<Task>) -> Result<()> {
        self.check_alive()?;
        let proc = Self::item_proc(task)?;
        self.try_push(
            self.gate.lane_index(&task.pid),
            QueueData::Task {
                task: task.clone(),
                proc,
            },
        )
    }

    /// Queue a task's `next` propagation. Same failure contract as
    /// [`Self::send`].
    pub(crate) fn send_next(&self, task: &Arc<Task>) -> Result<()> {
        self.check_alive()?;
        let proc = Self::item_proc(task)?;
        self.try_push(
            self.gate.lane_index(&task.pid),
            QueueData::Next {
                task: task.clone(),
                proc,
            },
        )
    }

    /// Queue one-hop error propagation. Same failure contract as
    /// [`Self::send`].
    pub(crate) fn send_error(&self, task: &Arc<Task>) -> Result<()> {
        self.check_alive()?;
        let proc = Self::item_proc(task)?;
        self.try_push(
            self.gate.lane_index(&task.pid),
            QueueData::Error {
                task: task.clone(),
                proc,
            },
        )
    }

    /// Queue one-hop abort propagation. Same failure contract as
    /// [`Self::send`].
    pub(crate) fn send_abort(&self, task: &Arc<Task>) -> Result<()> {
        self.check_alive()?;
        let proc = Self::item_proc(task)?;
        self.try_push(
            self.gate.lane_index(&task.pid),
            QueueData::AbortPropagation {
                task: task.clone(),
                proc,
            },
        )
    }

    /// The process lease a queued item carries. A task holds its process only
    /// `Weak`ly (see [`Task::proc`]), so a task clone that outlived an evicted
    /// process has nothing left to run it: refusing the item keeps the caller
    /// from reading `Ok` as "queued" — a recovery pass marks a durable outbox
    /// record `Dispatched` on `Ok`, which would stall work that never ran.
    fn item_proc(task: &Arc<Task>) -> Result<Arc<Process>> {
        task.proc().ok_or_else(|| {
            ActError::Runtime(format!(
                "cannot dispatch task '{}:{}': its process was deallocated",
                task.pid, task.id
            ))
        })
    }

    fn check_alive(&self) -> Result<()> {
        if !self.alive.load(Ordering::Acquire) {
            return Err(ActError::Shutdown);
        }
        Ok(())
    }

    /// Accept one item into its lane without blocking the producer. A full lane
    /// is reported to the caller so it can use the durable overflow path.
    fn try_push(&self, lane: usize, data: QueueData) -> Result<()> {
        if let Err(err) = self.senders[lane].try_send(data) {
            return Err(match err {
                mpsc::error::TrySendError::Full(_) => ActError::QueueFull,
                mpsc::error::TrySendError::Closed(_) => ActError::Shutdown,
            });
        }
        self.record_accepted();
        Ok(())
    }

    /// Mark the pool closed and wake every idle worker.
    pub fn abort(&self) {
        // Mark the pool closed before waking the workers. New producers see the
        // flag immediately; a worker parked in `recv` wakes on the sentinel. A
        // full lane needs no sentinel — its worker is running and leaves on the
        // closed flag when it comes back for the next item.
        self.alive.store(false, Ordering::Release);
        for lane in 0..self.senders.len() {
            let _ = self.try_push(lane, QueueData::Abort);
        }
    }

    /// Items waiting in the lanes right now, straight from the channels: a
    /// lane's backlog is its configured slots minus its free slots.
    fn buffered(&self) -> usize {
        self.senders
            .iter()
            .map(|sender| sender.max_capacity() - sender.capacity())
            .sum()
    }

    /// Sample the backlog where it grows — a successful admission is the only
    /// thing that can raise it — so the watermark is a real depth rather than
    /// an attempt count.
    fn record_accepted(&self) {
        self.high_watermark
            .fetch_max(self.buffered(), Ordering::AcqRel);
    }

    #[cfg(test)]
    fn push_for_test(&self, lane: usize, data: QueueData) -> Result<()> {
        self.try_push(lane, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lanes are the in-memory bound: a saturated lane refuses (so the
    /// caller can durably queue the work) instead of buffering without limit.
    #[tokio::test]
    async fn a_full_lane_refuses_instead_of_buffering() {
        let queue = Queue::new(4, ProcessGate::new(2));
        assert_eq!(queue.lanes(), 2);
        assert_eq!(queue.capacity(), 4);
        assert_eq!(queue.lane_capacity(), 2);

        queue.push_for_test(0, QueueData::Abort).unwrap();
        queue.push_for_test(0, QueueData::Abort).unwrap();
        assert_eq!(queue.depth(), 2);
        assert_eq!(queue.high_watermark(), 2);

        // the lane is full; the sibling lane is untouched and still accepts
        let err = queue.push_for_test(0, QueueData::Abort).unwrap_err();
        assert!(matches!(err, ActError::QueueFull));
        assert_eq!(queue.depth(), 2);
        queue.push_for_test(1, QueueData::Abort).unwrap();
        assert_eq!(queue.depth(), 3);

        // taking an item frees its slot — the backlog never exceeds the cap
        let mut receivers = queue.take_receivers();
        assert!(matches!(
            receivers[0].recv().await.unwrap(),
            QueueData::Abort
        ));
        assert_eq!(queue.depth(), 2);
        assert!(queue.high_watermark() <= queue.capacity());
    }
}

/// Marks the queue as unusable when its worker pool is gone. `Drop` runs during
/// panic propagation too.
#[derive(Debug)]
pub(crate) struct QueueConsumerLease {
    alive: Arc<AtomicBool>,
}

impl Drop for QueueConsumerLease {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
    }
}
