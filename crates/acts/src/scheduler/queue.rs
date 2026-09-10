use crate::ActError;
use crate::{
    Result,
    scheduler::{Process, Task},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::{Mutex, mpsc};

#[derive(Debug, Clone)]
pub struct Queue {
    receiver: Arc<Mutex<mpsc::UnboundedReceiver<QueueData>>>,
    sender: Arc<mpsc::UnboundedSender<QueueData>>,
    alive: Arc<AtomicBool>,
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
    pub fn new() -> Arc<Self> {
        let (tx, rx) = mpsc::unbounded_channel::<QueueData>();

        Arc::new(Self {
            receiver: Arc::new(Mutex::new(rx)),
            sender: Arc::new(tx),
            alive: Arc::new(AtomicBool::new(true)),
        })
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
        receiver
            .recv()
            .await
            .ok_or_else(|| ActError::Runtime("queue channel closed".to_string()))
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
        self.sender
            .send(QueueData::Task {
                task: task.clone(),
                proc,
            })
            .map_err(|err| ActError::Runtime(err.to_string()))?;
        Ok(())
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
        self.sender
            .send(QueueData::Next {
                task: task.clone(),
                proc,
            })
            .map_err(|err| ActError::Runtime(err.to_string()))?;
        Ok(())
    }

    pub fn abort(&self) {
        // Mark the queue closed before waking the consumer. New producers see
        // the flag immediately; the Abort item wakes a currently idle loop.
        self.alive.store(false, Ordering::Release);
        let _ = self.sender.send(QueueData::Abort);
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
