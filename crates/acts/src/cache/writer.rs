use std::sync::{Arc, Mutex, mpsc};

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
    /// Durable outbox enqueue: record the task's `next` as pending. Ordered
    /// after the task write queued by the same caller, so when this record
    /// becomes durable the task state it depends on is durable too.
    EnqueueNext {
        pid: String,
        tid: String,
    },
    /// Durable outbox enqueue: record a client action (event + options) as
    /// pending, before the action is applied in memory, so a crash before the
    /// task state write lands can replay the action on recovery.
    EnqueueAction {
        pid: String,
        tid: String,
        event: String,
        options: String,
    },
    /// Durable outbox close: mark the task's in-flight records of `r#type`
    /// `Done` — a `next` close must not sweep away a concurrent client-action
    /// record (and vice versa). Ordered after the task state write (and the
    /// message status), so `Done` is only durable once the effects are.
    OpDone {
        pid: String,
        tid: String,
        r#type: String,
    },
    Barrier(mpsc::SyncSender<()>),
}

#[derive(Clone)]
pub(crate) struct StoreWriter {
    tx: Arc<Mutex<Option<mpsc::Sender<WriteOp>>>>,
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

        Self {
            tx: Arc::new(Mutex::new(Some(tx))),
        }
    }

    pub(crate) fn send(&self, op: WriteOp) -> Result<()> {
        let tx = self.sender()?;
        tx.send(op)
            .map_err(|_| ActError::Runtime("store writer channel closed".to_string()))
    }

    /// Block until all previously enqueued writes have been applied.
    pub(crate) fn flush(&self) {
        let Ok(sender) = self.sender() else {
            return;
        };
        let (tx, rx) = mpsc::sync_channel(1);
        if sender.send(WriteOp::Barrier(tx)).is_err() {
            return;
        }
        let _ = rx.recv();
    }

    fn sender(&self) -> Result<mpsc::Sender<WriteOp>> {
        self.tx
            .lock()
            .unwrap()
            .as_ref()
            .cloned()
            .ok_or_else(|| ActError::Runtime("store writer channel closed".to_string()))
    }

    fn apply(store: &Store, op: WriteOp) -> Result<()> {
        match op {
            WriteOp::Task(task) => Self::apply_task(store, &task),
            WriteOp::MessageStatus { pid, tid, status } => {
                store.set_message_with(&pid, &tid, status)?;
                Ok(())
            }
            WriteOp::EnqueueNext { pid, tid } => {
                store.enqueue_next_op(&pid, &tid)?;
                Ok(())
            }
            WriteOp::EnqueueAction {
                pid,
                tid,
                event,
                options,
            } => {
                store.enqueue_action_op(&pid, &tid, &event, &options)?;
                Ok(())
            }
            WriteOp::OpDone { pid, tid, r#type } => {
                store.complete_ops(&pid, &tid, &r#type)?;
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
            store.mark_proc_complete(&task.pid, task.proc().end_time(), task.proc().state())?;
        }
        Ok(())
    }

    pub(crate) fn close(&self) {
        self.flush();
        self.tx.lock().unwrap().take();
    }
}
