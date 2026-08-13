use std::sync::{Arc, mpsc};

use tracing::error;

use crate::{ActError, Result, data, scheduler::TaskState, store::Store};

pub(crate) enum WriteOp {
    Task(data::Task),
    ProcComplete {
        pid: String,
        end_time: i64,
        state: TaskState,
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
            WriteOp::Task(data) => store.upsert_task_data(&data),
            WriteOp::ProcComplete {
                pid,
                end_time,
                state,
            } => store.mark_proc_complete(&pid, end_time, state),
            WriteOp::Barrier(tx) => {
                let _ = tx.send(());
                Ok(())
            }
        }
    }
}
