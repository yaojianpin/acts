use std::sync::{Arc, mpsc};

use tracing::error;

use crate::{ActError, Result, data::MessageStatus, scheduler::Task, store::Store};

pub(crate) enum WriteOp {
    /// Persist a task, its root task, and mark the process complete when needed.
    /// Serialization happens on the writer thread, off the caller's hot path.
    Task(Arc<Task>),
    /// Deferred message status update (marks a task's message as completed).
    MessageStatus {
        pid: String,
        tid: String,
        status: MessageStatus,
    },
    Barrier(mpsc::SyncSender<()>),
}

#[derive(Clone)]
pub(crate) struct StoreWriter {
    tx: mpsc::Sender<WriteOp>,
}

impl StoreWriter {
    pub(crate) fn spawn(store: Arc<Store>) -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(op) = rx.recv() {
                if let Err(err) = Self::apply(&store, op) {
                    error!("store writer error: {}", err);
                }
            }
        });

        Self { tx }
    }

    pub(crate) fn send(&self, op: WriteOp) -> Result<()> {
        self.tx
            .send(op)
            .map_err(|_| ActError::Runtime("store writer channel closed".to_string()))
    }

    /// Block until all previously enqueued writes have been applied.
    pub(crate) fn flush(&self) {
        let (tx, rx) = mpsc::sync_channel(1);
        if self.tx.send(WriteOp::Barrier(tx)).is_err() {
            return;
        }
        let _ = rx.recv();
    }

    fn apply(store: &Store, op: WriteOp) -> Result<()> {
        match op {
            WriteOp::Task(task) => Self::apply_task(store, &task),
            WriteOp::MessageStatus { pid, tid, status } => {
                store.set_message_with(&pid, &tid, status)?;
                Ok(())
            }
            WriteOp::Barrier(tx) => {
                let _ = tx.send(());
                Ok(())
            }
        }
    }

    fn apply_task(store: &Store, task: &Arc<Task>) -> Result<()> {
        store.upsert_task(task)?;
        if let Some(root) = task.proc().root() {
            store.upsert_task(&root)?;
        }
        if task.proc().state().is_completed() {
            store.mark_proc_complete(
                &task.pid,
                task.proc().end_time(),
                task.proc().state(),
            )?;
        }
        Ok(())
    }
}
