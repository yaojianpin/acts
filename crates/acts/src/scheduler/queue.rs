use crate::ActError;
use crate::{
    Result,
    scheduler::{Process, Task},
};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

#[derive(Debug, Clone)]
pub struct Queue {
    receiver: Arc<Mutex<mpsc::UnboundedReceiver<QueueData>>>,
    sender: Arc<mpsc::UnboundedSender<QueueData>>,
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
        })
    }

    pub async fn next(&self) -> Result<QueueData> {
        let mut receiver = self.receiver.lock().await;
        receiver
            .recv()
            .await
            .ok_or_else(|| ActError::Runtime("queue channel closed".to_string()))
    }

    pub(crate) fn send(&self, task: &Arc<Task>) -> Result<()> {
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
        self.sender
            .send(QueueData::Abort)
            .map_err(|err| ActError::Runtime(err.to_string()))
            .unwrap();
        // Drop all senders to close the channel, causing recv() to return None.
        drop(self.sender.clone());
    }
}
