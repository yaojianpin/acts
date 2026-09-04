use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;

use parking_lot::Mutex;
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
    /// Drop a process and its rows (tasks, outbox ops). Queued on the writer
    /// after any pending writes of the process, so removal can never race
    /// them: the completion markers apply first, then the rows are dropped.
    RemoveProc {
        pid: String,
    },
    Barrier(mpsc::SyncSender<Result<()>>),
}

#[derive(Clone)]
pub(crate) struct StoreWriter {
    tx: Arc<Mutex<Option<mpsc::Sender<WriteOp>>>>,
    thread: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl StoreWriter {
    pub(crate) fn spawn(store: Arc<Store>) -> Self {
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            // First failure of any write enqueued since the previous barrier.
            // Every failing write is logged as it happens; the next `flush()`
            // caller additionally learns about it through the barrier ack,
            // because a write that failed before the barrier is not durable.
            let mut failed: Option<ActError> = None;
            while let Ok(op) = rx.recv() {
                let res = match op {
                    WriteOp::Barrier(ack) => {
                        let _ = ack.send(failed.take().map_or(Ok(()), Err));
                        Ok(())
                    }
                    op => Self::apply(&store, op),
                };
                if let Err(err) = res {
                    error!("store writer error: {}", err);
                    if failed.is_none() {
                        failed = Some(err);
                    }
                }
            }
        });

        Self {
            tx: Arc::new(Mutex::new(Some(tx))),
            thread: Arc::new(Mutex::new(Some(thread))),
        }
    }

    pub(crate) fn send(&self, op: WriteOp) -> Result<()> {
        let tx = self.sender()?;
        tx.send(op)
            .map_err(|_| ActError::Runtime("store writer channel closed".to_string()))
    }

    /// Block until all previously enqueued writes have been applied.
    ///
    /// Returns the first failure of a write enqueued since the previous
    /// flush: a flush only acks `Ok` when every write queued before the
    /// barrier was applied successfully, so callers can rely on the data
    /// being durable.
    pub(crate) fn flush(&self) -> Result<()> {
        let sender = self.sender()?;
        let (tx, rx) = mpsc::sync_channel(1);
        sender
            .send(WriteOp::Barrier(tx))
            .map_err(|_| ActError::Runtime("store writer channel closed".to_string()))?;
        match rx.recv() {
            Ok(res) => res,
            Err(_) => Err(ActError::Runtime("store writer channel closed".to_string())),
        }
    }

    fn sender(&self) -> Result<mpsc::Sender<WriteOp>> {
        self.tx
            .lock()
            .as_ref()
            .cloned()
            .ok_or_else(|| ActError::Runtime("store writer channel closed".to_string()))
    }

    fn apply(store: &Store, op: WriteOp) -> Result<()> {
        match op {
            WriteOp::Task(task) => Self::apply_task(store, &task),
            WriteOp::MessageStatus { pid, tid, status } => {
                store.set_deliveries_with(&pid, &tid, status)?;
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
            WriteOp::RemoveProc { pid } => {
                store.remove_proc(&pid)?;
                Ok(())
            }
            // Acked by the writer loop before `apply`, never reached here.
            WriteOp::Barrier(_) => unreachable!("barrier is acked by the writer loop"),
        }
    }

    fn apply_task(store: &Store, task: &Arc<Task>) -> Result<()> {
        // A task write that reaches the writer after its process was already
        // removed is dead data. Removal is queued on the writer too (FIFO),
        // so every write enqueued before the removal has already been applied
        // by now; skipping the late write keeps it from re-creating rows or
        // failing (missing procs row) for a process that no longer exists.
        if !store.procs().exists(&task.pid)? {
            return Ok(());
        }
        store.upsert_task(task)?;
        if let Some(root) = task.proc().root() {
            store.upsert_task(&root)?;
        }
        if task.proc().state().is_completed() {
            store.mark_proc_complete(&task.pid, task.proc().end_time(), task.proc().state())?;
        }
        Ok(())
    }

    /// Flush every pending write, stop the writer thread and wait until it
    /// has fully exited. When this returns no writer thread is left running:
    /// every op enqueued before the thread stopped has been applied. Later
    /// `send`/`flush` calls fail with a channel-closed error, and calling
    /// `close` again is a no-op.
    pub(crate) fn close(&self) {
        // Failures of the drained writes were already logged by the writer
        // thread; do not let them abort the shutdown.
        let _ = self.flush();
        self.tx.lock().take();
        if let Some(thread) = self.thread.lock().take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{KvStore, MemoryStore, ScanOptions};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    /// Memory kv that can be switched, from the test thread, to fail every
    /// `put` (a store outage) or to block every `put` (holding the writer
    /// thread inside an in-flight write).
    struct TestKv {
        inner: MemoryStore,
        fail_put: AtomicBool,
        gate: AtomicBool,
        in_gate: AtomicBool,
    }

    impl TestKv {
        fn new() -> Self {
            Self {
                inner: MemoryStore::new(),
                fail_put: AtomicBool::new(false),
                gate: AtomicBool::new(false),
                in_gate: AtomicBool::new(false),
            }
        }

        fn set_fail(&self, fail: bool) {
            self.fail_put.store(fail, Ordering::SeqCst);
        }

        fn arm_gate(&self) {
            self.gate.store(true, Ordering::SeqCst);
        }

        fn disarm_gate(&self) {
            self.gate.store(false, Ordering::SeqCst);
        }

        fn wait_entered(&self) {
            while !self.in_gate.load(Ordering::SeqCst) {
                std::thread::yield_now();
            }
        }
    }

    impl KvStore for TestKv {
        fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
            self.inner.get(key)
        }

        fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
            if self.gate.load(Ordering::SeqCst) {
                self.in_gate.store(true, Ordering::SeqCst);
                while self.gate.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
            if self.fail_put.load(Ordering::SeqCst) {
                return Err(ActError::Runtime("injected put failure".to_string()));
            }
            self.inner.put(key, value)
        }

        fn delete(&self, key: &str) -> Result<()> {
            self.inner.delete(key)
        }

        fn scan_prefix(&self, key: &str, options: ScanOptions) -> Result<Vec<(String, Vec<u8>)>> {
            self.inner.scan_prefix(key, options)
        }
    }

    fn test_writer() -> (Arc<Store>, Arc<TestKv>, StoreWriter) {
        let kv = Arc::new(TestKv::new());
        let store = Arc::new(Store::new(kv.clone()));
        let writer = StoreWriter::spawn(store.clone());
        (store, kv, writer)
    }

    fn enqueue(writer: &StoreWriter, pid: &str) {
        writer
            .send(WriteOp::EnqueueNext {
                pid: pid.to_string(),
                tid: "t1".to_string(),
            })
            .unwrap();
    }

    fn durable(store: &Store, pid: &str) -> bool {
        store
            .load_pending_ops()
            .unwrap()
            .iter()
            .any(|op| op.pid == pid)
    }

    /// `flush` acks `Ok` only when every write queued before the barrier was
    /// applied: a failing write is reported to the caller that flushes.
    #[test]
    fn flush_reports_earlier_write_failure_and_recovers() {
        let (store, kv, writer) = test_writer();

        // healthy write lands
        enqueue(&writer, "ok1");
        writer.flush().unwrap();
        assert!(durable(&store, "ok1"));

        // store outage: queued writes fail, and the next flush surfaces it
        // instead of silently acking `Ok`
        kv.set_fail(true);
        enqueue(&writer, "lost1");
        enqueue(&writer, "lost2");
        let err = writer.flush().unwrap_err();
        assert!(
            err.to_string().contains("injected"),
            "flush should report the earlier write failure, got: {err}"
        );
        assert!(!durable(&store, "lost1"));
        assert!(!durable(&store, "lost2"));

        // outage over: the failure was consumed by the flush, later flushes
        // are clean and later writes are durable
        kv.set_fail(false);
        enqueue(&writer, "ok2");
        writer.flush().unwrap();
        assert!(durable(&store, "ok2"));
    }

    /// A flush with nothing failing acks cleanly even when the queue is empty.
    #[test]
    fn flush_is_clean_without_failures() {
        let (_, _, writer) = test_writer();
        writer.flush().unwrap();
    }

    /// `close` flushes pending writes first, waits for an in-flight write to
    /// finish, and joins the writer thread, so nothing is left running when
    /// it returns.
    #[test]
    fn close_waits_for_in_flight_write_and_joins_the_thread() {
        let (store, kv, writer) = test_writer();

        // hold the writer inside a write so it cannot drain while close runs
        kv.arm_gate();
        enqueue(&writer, "p1");
        kv.wait_entered();

        let closer = {
            let writer = writer.clone();
            std::thread::spawn(move || writer.close())
        };

        // close() flushes first, so it must not return while the write is
        // still in flight
        std::thread::sleep(Duration::from_millis(50));
        assert!(
            !closer.is_finished(),
            "close() returned while a write was in flight"
        );

        kv.disarm_gate();
        closer.join().unwrap();

        // the in-flight write was drained before close() returned
        assert!(durable(&store, "p1"));
        // and the writer thread was joined: nothing is left running
        assert!(
            writer.thread.lock().is_none(),
            "writer thread was not joined by close()"
        );
    }

    /// After `close` the writer is gone: further sends fail, and `close` is
    /// idempotent.
    #[test]
    fn send_and_flush_fail_after_close() {
        let (store, _, writer) = test_writer();
        enqueue(&writer, "p1");
        writer.close();
        assert!(durable(&store, "p1"));

        writer.close(); // no-op

        let send_err = writer
            .send(WriteOp::EnqueueNext {
                pid: "p2".to_string(),
                tid: "t1".to_string(),
            })
            .unwrap_err();
        assert!(send_err.to_string().contains("closed"), "{send_err}");
        let flush_err = writer.flush().unwrap_err();
        assert!(flush_err.to_string().contains("closed"), "{flush_err}");
    }

    /// `RemoveProc` is applied FIFO after the writes queued before it, then
    /// deletes the process's outbox records; one flush covers both and
    /// reports no failure. Removing an absent process is a no-op.
    #[test]
    fn remove_proc_deletes_outbox_rows_after_pending_writes() {
        let (store, _, writer) = test_writer();

        // the enqueue is queued before the removal: it applies first, then
        // its rows are dropped by the removal
        enqueue(&writer, "p1");
        writer
            .send(WriteOp::RemoveProc {
                pid: "p1".to_string(),
            })
            .unwrap();
        writer.flush().unwrap();
        assert!(
            !durable(&store, "p1"),
            "RemoveProc must drop the op rows of the process"
        );

        // removing an absent process is not an error
        writer
            .send(WriteOp::RemoveProc {
                pid: "p1".to_string(),
            })
            .unwrap();
        writer.flush().unwrap();
    }
}
