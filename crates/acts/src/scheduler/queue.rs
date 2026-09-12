use crate::ActError;
use crate::{
    Result,
    scheduler::{Process, Task},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::sync::{Mutex, mpsc};

#[derive(Debug, Clone)]
pub struct Queue {
    receiver: Arc<Mutex<QueueReceiver>>,
    sender: QueueSender,
    alive: Arc<AtomicBool>,
    depth: Arc<AtomicUsize>,
    high_watermark: Arc<AtomicUsize>,
    capacity: usize,
}

#[derive(Debug)]
pub enum QueueData {
    Task {
        task: Arc<Task>,
        /// Execution-time lease: a queued task must remain executable even if
        /// its finished process is evicted before the loop reaches this item.
        proc: Arc<Process>,
    },
    Next {
        task: Arc<Task>,
        proc: Arc<Process>,
    },
    Abort,
}

impl Queue {
    pub fn new(capacity: usize) -> Arc<Self> {
        let capacity = capacity.max(1);
        let (tx, rx) = mpsc::channel::<QueueData>(capacity);

        Arc::new(Self {
            receiver: Arc::new(Mutex::new(QueueReceiver::Bounded(rx))),
            sender: QueueSender::Bounded(tx),
            alive: Arc::new(AtomicBool::new(true)),
            depth: Arc::new(AtomicUsize::new(0)),
            high_watermark: Arc::new(AtomicUsize::new(0)),
            capacity,
        })
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Current number of items buffered in memory (not including an in-flight
    /// item already taken by the event loop).
    pub fn depth(&self) -> usize {
        self.depth.load(Ordering::Acquire)
    }

    pub fn high_watermark(&self) -> usize {
        self.high_watermark.load(Ordering::Acquire)
    }

    /// Pin the event loop as this queue's consumer. If that loop dies — even
    /// by an unexpected unwind — the lease makes later `send`s fail instead of
    /// silently accumulating work for a scheduler that will never run it.
    pub(crate) fn consumer_lease(&self) -> QueueConsumerLease {
        QueueConsumerLease {
            alive: self.alive.clone(),
        }
    }

    pub async fn next(&self) -> Result<QueueData> {
        let mut receiver = self.receiver.lock().await;
        let data = receiver
            .recv()
            .await
            .ok_or_else(|| ActError::Runtime("queue channel closed".to_string()))?;
        self.record_released();
        Ok(data)
    }

    pub(crate) fn send(&self, task: &Arc<Task>) -> Result<()> {
        if !self.alive.load(Ordering::Acquire) {
            return Err(ActError::Runtime(
                "scheduler queue consumer is not running".to_string(),
            ));
        }
        let Some(proc) = task.proc() else {
            return Ok(());
        };
        self.send_data(QueueData::Task {
            task: task.clone(),
            proc,
        })
    }

    pub(crate) fn send_next(&self, task: &Arc<Task>) -> Result<()> {
        if !self.alive.load(Ordering::Acquire) {
            return Err(ActError::Runtime(
                "scheduler queue consumer is not running".to_string(),
            ));
        }
        let Some(proc) = task.proc() else {
            return Ok(());
        };
        self.send_data(QueueData::Next {
            task: task.clone(),
            proc,
        })
    }

    fn send_data(&self, data: QueueData) -> Result<()> {
        if !self.alive.load(Ordering::Acquire) {
            return Err(ActError::Runtime(
                "scheduler queue consumer is not running".to_string(),
            ));
        }
        self.record_accepted();
        if let Err(err) = self.sender.send(data) {
            self.record_released();
            return Err(err);
        }
        Ok(())
    }

    pub fn abort(&self) {
        // Mark the queue closed before waking the consumer. New producers see
        // the flag immediately; the Abort item wakes a currently idle loop.
        self.alive.store(false, Ordering::Release);
        if self.sender.send(QueueData::Abort).is_ok() {
            self.record_accepted();
        }
    }

    fn record_accepted(&self) {
        let depth = self.depth.fetch_add(1, Ordering::AcqRel) + 1;
        self.high_watermark.fetch_max(depth, Ordering::AcqRel);
    }

    fn record_released(&self) {
        let _ = self
            .depth
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |depth| {
                Some(depth.saturating_sub(1))
            });
    }
}

#[derive(Debug, Clone)]
enum QueueSender {
    Bounded(mpsc::Sender<QueueData>),
}

impl QueueSender {
    /// Accept one item without blocking the scheduler. A full bounded queue is
    /// reported to the caller so it can use the durable overflow path.
    fn send(&self, data: QueueData) -> Result<()> {
        match self {
            QueueSender::Bounded(tx) => tx.try_send(data).map_err(|err| match err {
                mpsc::error::TrySendError::Full(_) => ActError::QueueFull,
                mpsc::error::TrySendError::Closed(_) => {
                    ActError::Runtime("scheduler queue channel closed".to_string())
                }
            }),
        }
    }
}

#[derive(Debug)]
enum QueueReceiver {
    Bounded(mpsc::Receiver<QueueData>),
}

impl QueueReceiver {
    async fn recv(&mut self) -> Option<QueueData> {
        match self {
            QueueReceiver::Bounded(rx) => rx.recv().await,
        }
    }
}

#[cfg(test)]
impl Queue {
    fn send_for_test(&self, data: QueueData) -> Result<()> {
        self.send_data(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bounded_queue_rejects_when_full() {
        let queue = Queue::new(1);
        queue.send_for_test(QueueData::Abort).unwrap();
        assert_eq!(queue.depth(), 1);
        assert_eq!(queue.high_watermark(), 1);

        let err = queue.send_for_test(QueueData::Abort).unwrap_err();
        assert!(matches!(err, ActError::QueueFull));
        assert_eq!(queue.depth(), 1);

        assert!(matches!(queue.next().await.unwrap(), QueueData::Abort));
        assert_eq!(queue.depth(), 0);
    }
}

/// Marks the queue as unusable when the event-loop future is dropped or
/// unwinds. `Drop` runs during panic propagation too.
#[derive(Debug)]
pub(crate) struct QueueConsumerLease {
    alive: Arc<AtomicBool>,
}

impl Drop for QueueConsumerLease {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
    }
}
