use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use parking_lot::Mutex;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::error;

use crate::{ActError, Result, data::DeliveryStatus, scheduler::Task, store::Store, utils::consts};

pub(crate) enum WriteOp {
    /// Persist a task, its root task, and mark the process complete when needed.
    /// Serialization happens on the writer thread, off the caller's hot path.
    Task(Arc<Task>),
    /// Deferred delivery-status update: closes every delivery row of a
    /// finished task's message (the client is not asked to act again).
    DeliveryStatus {
        pid: String,
        tid: String,
        status: DeliveryStatus,
    },
    /// Durable outbox enqueue: record the task's `next` as pending. Ordered
    /// after the task write queued by the same caller, so when this record
    /// becomes durable the task state it depends on is durable too.
    EnqueueNext {
        pid: String,
        tid: String,
    },
    /// Durable outbox enqueue: record a task execution as pending when the
    /// in-memory scheduler queue is full. The task state is queued before this
    /// record, so replay always finds a durable task.
    EnqueueExec {
        pid: String,
        tid: String,
    },
    /// Mark an outbox record as handed to the in-memory scheduler. A crash can
    /// still replay it: boot recovery treats `Dispatched` like `Pending`.
    MarkOpDispatched {
        pid: String,
        tid: String,
        r#type: String,
    },
    /// Mark a `next` record as overflowed after bounded queue rejection.
    MarkOpOverflow {
        pid: String,
        tid: String,
        r#type: String,
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
    /// Drop a process and its rows (tasks, outbox ops, message/delivery rows).
    /// Queued on the writer after any pending writes of the process, so
    /// removal can never race them: the completion markers apply first, then
    /// the rows are dropped.
    RemoveProc {
        pid: String,
    },
    Barrier(oneshot::Sender<Result<()>>),
}

#[derive(Clone)]
pub(crate) struct StoreWriter {
    tx: Arc<Mutex<Option<mpsc::UnboundedSender<WriteOp>>>>,
    task: Arc<Mutex<Option<JoinHandle<()>>>>,
    depth: Arc<AtomicUsize>,
    high_watermark: Arc<AtomicUsize>,
}

impl StoreWriter {
    pub(crate) fn spawn(store: Arc<Store>) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<WriteOp>();
        let depth = Arc::new(AtomicUsize::new(0));
        let high_watermark = Arc::new(AtomicUsize::new(0));
        let writer_depth = depth.clone();
        // Runs on the ambient tokio runtime. Ordering is preserved: a single
        // consumer applies the ops in FIFO order, so the durability
        // guarantees (task state durable before outbox records, removal after
        // every pending write of the process) are unchanged.
        let task = tokio::spawn(async move {
            // First failure of any write enqueued since the previous barrier.
            // Every failing write is logged as it happens; the next `flush()`
            // caller additionally learns about it through the barrier ack,
            // because a write that failed before the barrier is not durable.
            let mut failed: Option<ActError> = None;
            while let Some(op) = rx.recv().await {
                Self::record_released(&writer_depth);
                let res = match op {
                    WriteOp::Barrier(ack) => {
                        let _ = ack.send(failed.take().map_or(Ok(()), Err));
                        Ok(())
                    }
                    op => Self::apply(&store, op).await,
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
            task: Arc::new(Mutex::new(Some(task))),
            depth,
            high_watermark,
        }
    }

    /// Async producer backpressure: wait until the bounded writer queue has a
    /// slot. This is the database-style write-stall path for async callers.
    pub(crate) async fn send(&self, op: WriteOp) -> Result<()> {
        let tx = self.sender()?;
        self.record_accepted();
        tx.send(op)
            .map_err(|_| ActError::Runtime("store writer channel closed".to_string()))?;
        Ok(())
    }

    /// Non-blocking admission for producers that cannot await. A full bounded
    /// queue is an explicit overload error; it never silently grows memory.
    pub(crate) fn try_send(&self, op: WriteOp) -> Result<()> {
        let tx = self.sender()?;
        self.record_accepted();
        tx.send(op).map_err(|_| {
            Self::record_released(&self.depth);
            ActError::Runtime("store writer channel closed".to_string())
        })?;
        Ok(())
    }

    fn record_accepted(&self) {
        let depth = self.depth.fetch_add(1, Ordering::AcqRel) + 1;
        let watermark = self.high_watermark.fetch_max(depth, Ordering::AcqRel);
        if depth == 1024 || (depth > 1024 && depth.is_power_of_two()) {
            error!(
                depth,
                high_watermark = watermark.max(depth),
                "store writer backlog is high"
            );
        }
    }

    pub(crate) fn depth(&self) -> usize {
        self.depth.load(Ordering::Acquire)
    }

    fn record_released(depth: &AtomicUsize) {
        let _ = depth.fetch_update(Ordering::AcqRel, Ordering::Acquire, |depth| {
            Some(depth.saturating_sub(1))
        });
    }

    pub(crate) fn high_watermark(&self) -> usize {
        self.high_watermark.load(Ordering::Acquire)
    }

    /// Block until all previously enqueued writes have been applied.
    ///
    /// Returns the first failure of a write enqueued since the previous
    /// flush: a flush only acks `Ok` when every write queued before the
    /// barrier was applied successfully, so callers can rely on the data
    /// being durable.
    pub(crate) async fn flush(&self) -> Result<()> {
        let sender = self.sender()?;
        let (tx, rx) = oneshot::channel();
        self.record_accepted();
        sender
            .send(WriteOp::Barrier(tx))
            .map_err(|_| ActError::Runtime("store writer channel closed".to_string()))?;
        rx.await
            .map_err(|_| ActError::Runtime("store writer task dropped".to_string()))?
    }

    fn sender(&self) -> Result<mpsc::UnboundedSender<WriteOp>> {
        self.tx
            .lock()
            .as_ref()
            .cloned()
            .ok_or_else(|| ActError::Runtime("store writer channel closed".to_string()))
    }

    async fn apply(store: &Store, op: WriteOp) -> Result<()> {
        match op {
            WriteOp::Task(task) => Self::apply_task(store, &task).await,
            WriteOp::DeliveryStatus { pid, tid, status } => {
                store.set_deliveries_with(&pid, &tid, status).await?;
                // the task close may have settled the process's last open
                // delivery — mark it removable when the process is finished
                // and nothing is left open
                let _ = store.try_mark_removable(&pid).await;
                Ok(())
            }
            WriteOp::EnqueueNext { pid, tid } => {
                store.enqueue_next_op(&pid, &tid).await?;
                Ok(())
            }
            WriteOp::EnqueueExec { pid, tid } => {
                store.enqueue_exec_op(&pid, &tid).await?;
                Ok(())
            }
            WriteOp::MarkOpDispatched { pid, tid, r#type } => {
                store.mark_op_dispatched(&pid, &tid, &r#type).await?;
                Ok(())
            }
            WriteOp::MarkOpOverflow { pid, tid, r#type } => {
                store.mark_op_overflow(&pid, &tid, &r#type).await?;
                Ok(())
            }
            WriteOp::EnqueueAction {
                pid,
                tid,
                event,
                options,
            } => {
                store
                    .enqueue_action_op(&pid, &tid, &event, &options)
                    .await?;
                Ok(())
            }
            WriteOp::OpDone { pid, tid, r#type } => {
                store.complete_ops(&pid, &tid, &r#type).await?;
                Ok(())
            }
            WriteOp::RemoveProc { pid } => {
                store.remove_proc(&pid).await?;
                Ok(())
            }
            // Acked by the writer loop before `apply`, never reached here.
            WriteOp::Barrier(_) => unreachable!("barrier is acked by the writer loop"),
        }
    }

    async fn apply_task(store: &Store, task: &Arc<Task>) -> Result<()> {
        // A task write that reaches the writer after its process was already
        // removed is dead data. Removal is queued on the writer too (FIFO),
        // so every write enqueued before the removal has already been applied
        // by now; skipping the late write keeps it from re-creating rows or
        // failing (missing procs row) for a process that no longer exists.
        if !store.procs().exists(&task.pid).await? {
            return Ok(());
        }
        // lifecycle row + the vars rows of every dirty scope on the parent
        // chain (scope vars are decoupled from task state writes). FIFO order
        // keeps the scope vars (e.g. the `NEXT_COMPLETE` marker) durable
        // before any outbox record queued after this write.
        store.persist_task_rows(task).await?;
        if let Some(p) = task.proc() {
            if p.state().is_completed() {
                store
                    .mark_proc_complete(&task.pid, p.end_time(), p.state())
                    .await?;
            }
        } else if task.id == consts::TASK_ROOT_TID && task.state().is_completed() {
            // The process instance is already gone (evicted and deallocated
            // while this write was queued) — the root task's own state is the
            // faithful terminal stamp, so the proc row still settles and the
            // sweeper can remove it
            store
                .mark_proc_complete(&task.pid, task.end_time(), task.state())
                .await?;
        }
        // A message is done when it has no delivery rows (its own state is
        // terminal — the message state is a projection of the task state) or
        // when every delivery of it has settled. The task's terminal write is
        // the authoritative point for both:
        //  1. close the task's own delivery rows `Completed` — the client is
        //     not asked to act on a finished task (this covers tasks
        //     completed by the engine itself, with no client action ever);
        //  2. re-check the removable mark — a process with no delivery rows
        //     (or all settled) is marked here, so the sweeper deletes it.
        if task.state().is_completed() {
            store
                .set_deliveries_with(&task.pid, &task.id, DeliveryStatus::Completed)
                .await?;
            let _ = store.try_mark_removable(&task.pid).await;
        }
        Ok(())
    }

    /// Flush every pending write, stop the writer thread and wait until it
    /// has fully exited. When this returns no writer thread is left running:
    /// every op enqueued before the thread stopped has been applied. Later
    /// `send`/`flush` calls fail with a channel-closed error, and calling
    /// `close` again is a no-op.
    pub(crate) async fn close(&self) {
        // Failures of the drained writes were already logged by the writer
        // task; do not let them abort the shutdown.
        let _ = self.flush().await;
        self.tx.lock().take();
        let task = { self.task.lock().take() };
        if let Some(task) = task {
            let _ = task.await;
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
    /// task inside an in-flight write).
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

        /// Yield until the writer task is parked inside a gated `put`.
        async fn wait_entered(&self) {
            while !self.in_gate.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        }
    }

    #[async_trait::async_trait]
    impl KvStore for TestKv {
        async fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
            self.inner.get(key).await
        }

        async fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
            if self.gate.load(Ordering::SeqCst) {
                self.in_gate.store(true, Ordering::SeqCst);
                while self.gate.load(Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            }
            if self.fail_put.load(Ordering::SeqCst) {
                return Err(ActError::Runtime("injected put failure".to_string()));
            }
            self.inner.put(key, value).await
        }

        async fn delete(&self, key: &str) -> Result<()> {
            self.inner.delete(key).await
        }

        async fn scan_prefix(
            &self,
            key: &str,
            options: ScanOptions,
        ) -> Result<Vec<(String, Vec<u8>)>> {
            self.inner.scan_prefix(key, options).await
        }
    }

    fn test_writer() -> (Arc<Store>, Arc<TestKv>, StoreWriter) {
        let kv = Arc::new(TestKv::new());
        let store = Arc::new(Store::new(kv.clone()));
        let writer = StoreWriter::spawn(store.clone());
        (store, kv, writer)
    }

    async fn enqueue(writer: &StoreWriter, pid: &str) {
        writer
            .send(WriteOp::EnqueueNext {
                pid: pid.to_string(),
                tid: "t1".to_string(),
            })
            .await
            .unwrap();
    }

    async fn durable(store: &Store, pid: &str) -> bool {
        store
            .load_pending_ops()
            .await
            .unwrap()
            .iter()
            .any(|op| op.pid == pid)
    }

    /// `flush` acks `Ok` only when every write queued before the barrier was
    /// applied: a failing write is reported to the caller that flushes.
    #[tokio::test]
    async fn flush_reports_earlier_write_failure_and_recovers() {
        let (store, kv, writer) = test_writer();

        // healthy write lands
        enqueue(&writer, "ok1").await;
        writer.flush().await.unwrap();
        assert!(durable(&store, "ok1").await);

        // store outage: queued writes fail, and the next flush surfaces it
        // instead of silently acking `Ok`
        kv.set_fail(true);
        enqueue(&writer, "lost1").await;
        enqueue(&writer, "lost2").await;
        let err = writer.flush().await.unwrap_err();
        assert!(
            err.to_string().contains("injected"),
            "flush should report the earlier write failure, got: {err}"
        );
        assert!(!durable(&store, "lost1").await);
        assert!(!durable(&store, "lost2").await);

        // outage over: the failure was consumed by the flush, later flushes
        // are clean and later writes are durable
        kv.set_fail(false);
        enqueue(&writer, "ok2").await;
        writer.flush().await.unwrap();
        assert!(durable(&store, "ok2").await);
    }

    /// A flush with nothing failing acks cleanly even when the queue is empty.
    #[tokio::test]
    async fn flush_is_clean_without_failures() {
        let (_, _, writer) = test_writer();
        writer.flush().await.unwrap();
    }

    /// `close` flushes pending writes first, waits for an in-flight write to
    /// finish, and joins the writer task, so nothing is left running when it
    /// returns.
    #[tokio::test]
    async fn close_waits_for_in_flight_write_and_joins_the_thread() {
        let (store, kv, writer) = test_writer();

        // hold the writer inside a write so it cannot drain while close runs
        kv.arm_gate();
        enqueue(&writer, "p1").await;
        kv.wait_entered().await;

        let closer = {
            let writer = writer.clone();
            tokio::spawn(async move { writer.close().await })
        };

        // close() flushes first, so it must not return while the write is
        // still in flight
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !closer.is_finished(),
            "close() returned while a write was in flight"
        );

        kv.disarm_gate();
        closer.await.unwrap();

        // the in-flight write was drained before close() returned
        assert!(durable(&store, "p1").await);
        // and the writer task was joined: nothing is left running
        assert!(
            writer.task.lock().is_none(),
            "writer task was not joined by close()"
        );
    }

    /// After `close` the writer is gone: further sends fail, and `close` is
    /// idempotent.
    #[tokio::test]
    async fn send_and_flush_fail_after_close() {
        let (store, _, writer) = test_writer();
        enqueue(&writer, "p1").await;
        writer.close().await;
        assert!(durable(&store, "p1").await);

        writer.close().await; // no-op

        let send_err = writer
            .send(WriteOp::EnqueueNext {
                pid: "p2".to_string(),
                tid: "t1".to_string(),
            })
            .await
            .unwrap_err();
        assert!(send_err.to_string().contains("closed"), "{send_err}");
        let flush_err = writer.flush().await.unwrap_err();
        assert!(flush_err.to_string().contains("closed"), "{flush_err}");
    }

    /// `RemoveProc` is applied FIFO after the writes queued before it, then
    /// deletes the process's outbox records; one flush covers both and
    /// reports no failure. Removing an absent process is a no-op.
    #[tokio::test]
    async fn remove_proc_deletes_outbox_rows_after_pending_writes() {
        let (store, _, writer) = test_writer();

        // the enqueue is queued before the removal: it applies first, then
        // its rows are dropped by the removal
        enqueue(&writer, "p1").await;
        writer
            .send(WriteOp::RemoveProc {
                pid: "p1".to_string(),
            })
            .await
            .unwrap();
        writer.flush().await.unwrap();
        assert!(
            !durable(&store, "p1").await,
            "RemoveProc must drop the op rows of the process"
        );

        // removing an absent process is not an error
        writer
            .send(WriteOp::RemoveProc {
                pid: "p1".to_string(),
            })
            .await
            .unwrap();
        writer.flush().await.unwrap();
    }

    /// Writer depth is the number of accepted-but-not-consumed operations;
    /// in-flight writes are not part of depth. The watermark stays observable.
    #[tokio::test]
    async fn writer_tracks_backlog_watermark() {
        let kv = Arc::new(TestKv::new());
        let store = Arc::new(Store::new(kv.clone()));
        let writer = StoreWriter::spawn(store.clone());

        kv.arm_gate();
        enqueue(&writer, "p1").await;
        kv.wait_entered().await;
        assert_eq!(writer.depth(), 0); // the first op is in flight, not queued

        enqueue(&writer, "p2").await;
        assert_eq!(writer.depth(), 1);

        kv.disarm_gate();
        writer.flush().await.unwrap();
        assert_eq!(writer.depth(), 0);
        assert!(writer.high_watermark() >= 1);
    }
}
