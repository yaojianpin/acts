use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use parking_lot::Mutex;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use crate::{
    ActError, Result,
    scheduler::Task,
    store::Store,
    utils::{consts, pid_lane},
};

pub(crate) enum WriteOp {
    /// Persist a task, its root task, and mark the process complete when needed.
    /// Serialization happens on the writer thread, off the caller's hot path.
    Task(Arc<Task>),
    /// Deferred delivery close: settles every engine-owned delivery row of a
    /// finished task (`Created`/`Delivered`/`Acked` → `Completed`; an `Error`
    /// row stays for manual handling).
    CloseDeliveries {
        pid: String,
        tid: String,
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

impl WriteOp {
    /// The process this op belongs to. Every op of one pid is applied by the
    /// same shard in enqueue order — the order every durability guarantee
    /// documented above depends on, since all of them are within one process.
    /// Only the barrier belongs to no process: `flush` queues one behind every
    /// shard's backlog, so it is acknowledged once the whole backlog is
    /// applied.
    fn pid(&self) -> Option<&str> {
        match self {
            WriteOp::Task(task) => Some(task.pid.as_str()),
            WriteOp::CloseDeliveries { pid, .. }
            | WriteOp::EnqueueNext { pid, .. }
            | WriteOp::EnqueueExec { pid, .. }
            | WriteOp::MarkOpDispatched { pid, .. }
            | WriteOp::MarkOpOverflow { pid, .. }
            | WriteOp::EnqueueAction { pid, .. }
            | WriteOp::OpDone { pid, .. }
            | WriteOp::RemoveProc { pid } => Some(pid.as_str()),
            // Only the barrier has no process: `flush` queues one on every
            // shard, so it acks once the whole backlog is applied.
            WriteOp::Barrier(_) => None,
        }
    }
}

/// The write path's backlog accounting, shared by every producer and by the
/// consumers: how much was accepted but not applied, its high watermark, and
/// the saturation latch that keeps a backed-up store from being fed more work.
struct Backlog {
    /// Total ops the shards hold at once.
    capacity: usize,
    /// Ops accepted but not yet consumed by a shard's consumer.
    depth: AtomicUsize,
    high_watermark: AtomicUsize,
    /// Latched while the store is not keeping up — a shard had no room for an
    /// op. New work is refused until the backlog drains (see [`StoreWriter`]).
    saturated: AtomicBool,
}

impl Backlog {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            depth: AtomicUsize::new(0),
            high_watermark: AtomicUsize::new(0),
            saturated: AtomicBool::new(false),
        }
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

    fn record_released(&self) {
        let previous = self
            .depth
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |depth| {
                Some(depth.saturating_sub(1))
            })
            .unwrap_or_else(|depth| depth);
        // Re-open the writer to new work only once most of the backlog is
        // gone: a single free slot right after a refusal would just refuse the
        // next op too, and each refusal is an error the caller has to act on.
        if previous.saturating_sub(1) * 2 <= self.capacity
            && self.saturated.swap(false, Ordering::AcqRel)
        {
            info!("store writer backlog drained, accepting new work again");
        }
    }

    /// Whether the write path is currently refusing new work.
    fn is_saturated(&self) -> bool {
        self.saturated.load(Ordering::Acquire)
    }

    /// The store is behind: refuse new work until the backlog drains.
    fn latch(&self) {
        if !self.saturated.swap(true, Ordering::AcqRel) {
            warn!(
                depth = self.depth.load(Ordering::Acquire),
                capacity = self.capacity,
                "store writer is saturated: refusing new work until the backlog drains"
            );
        }
    }
}

/// The store's durable write path, sharded by pid: one FIFO queue and one
/// consumer per shard, and every op of a pid landing in the same shard. Ops of
/// one process therefore keep their enqueue order while independent processes
/// are applied concurrently — a slow write for one process no longer stalls
/// every other process's writes behind it.
///
/// Queues are bounded and nothing is ever buffered without limit. The path
/// distinguishes work that is *being admitted* from the bookkeeping of work
/// already in flight, and a saturated store changes what each does:
///
/// - [`Self::try_send`] admits new work (a task's own state write, or the
///   durable outbox record that stands in for it). A saturated writer — or a
///   shard with no room left — **refuses** the op with
///   [`ActError::QueueFull`] instead of queueing or waiting: new tasks stop
///   being written while the store is behind, so no backlog of doomed writes
///   accumulates. The caller's [`ActError::QueueFull`] handling routes the
///   work to its durable overflow path, or surfaces the overload.
/// - [`Self::send`] queues the bookkeeping of work already admitted (task
///   state transitions, outbox records and their closes, delivery closes,
///   removals). These ops are what makes an admitted operation durable, so they
///   are never refused — they wait for room, stalling the process they belong
///   to rather than losing its durability.
/// - [`Self::flush`] queues a barrier on every shard and waits for room, so a
///   flush always completes once the backlog drains.
///
/// The saturation latch is shared: a refusal (or a wait for room) latches it,
/// and it releases once the backlog is back under half the capacity. While
/// latched, new work is refused immediately even when the target shard has
/// room, instead of being admitted into a writer that is clearly behind.
#[derive(Clone)]
pub(crate) struct StoreWriter {
    /// One sender per shard, indexed by the pid hash. `None` once `close` took
    /// them, which is what makes later `send`/`flush` calls fail.
    ///
    /// Each queue is bounded by `capacity / shards` ops (at least one).
    senders: Arc<Mutex<Option<Vec<mpsc::Sender<WriteOp>>>>>,
    /// One consumer per shard; `close` joins them all.
    tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
    backlog: Arc<Backlog>,
}

impl StoreWriter {
    /// Spawn `shards` consumers (at least one), each with its own queue bounded
    /// by `capacity / shards` ops (at least one). They run on the ambient
    /// tokio runtime.
    pub(crate) fn spawn(store: Arc<Store>, shards: usize, capacity: usize) -> Self {
        let shards = shards.max(1);
        let lane_capacity = (capacity.max(1) / shards).max(1);
        let backlog = Arc::new(Backlog::new(lane_capacity * shards));
        let mut senders = Vec::with_capacity(shards);
        let mut tasks = Vec::with_capacity(shards);
        for _ in 0..shards {
            let (tx, mut rx) = mpsc::channel::<WriteOp>(lane_capacity);
            let store = store.clone();
            let backlog = backlog.clone();
            tasks.push(tokio::spawn(async move {
                // A consumer applies its shard's ops in FIFO order. Ordering is
                // preserved across the whole writer: every op of a process is
                // routed to the same shard, so the durability guarantees (task
                // state durable before the outbox records that depend on it,
                // removal after every pending write of the process) hold
                // exactly as they did with one global queue, while ops of
                // different processes are applied concurrently.
                //
                // First failure of any write enqueued since the previous
                // barrier. Every failing write is logged as it happens; the
                // next `flush()` caller additionally learns about it through
                // the barrier ack, because a write that failed before the
                // barrier is not durable.
                let mut failed: Option<ActError> = None;
                while let Some(op) = rx.recv().await {
                    backlog.record_released();
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
            }));
            senders.push(tx);
        }

        Self {
            senders: Arc::new(Mutex::new(Some(senders))),
            tasks: Arc::new(Mutex::new(tasks)),
            backlog,
        }
    }

    /// Admit new work into its process's shard without waiting: a saturated
    /// writer — or a shard with no room left — refuses with
    /// [`ActError::QueueFull`] and latches the writer, so a backed-up store is
    /// not fed more work and nothing accumulates waiting for room. The caller
    /// turns the refusal into its durable overflow path (or reports the
    /// overload); nothing is queued on its behalf.
    pub(crate) fn try_send(&self, op: WriteOp) -> Result<()> {
        if self.backlog.is_saturated() {
            return Err(ActError::QueueFull);
        }
        let tx = self.route(&op)?;
        match tx.try_send(op) {
            Ok(()) => {
                self.backlog.record_accepted();
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.backlog.latch();
                Err(ActError::QueueFull)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => Err(Self::closed()),
        }
    }

    /// Queue the bookkeeping write of already-admitted work, waiting for room
    /// when its shard is full. These ops are what makes an admitted operation
    /// durable, so they are never refused: the wait stalls the process they
    /// belong to instead of losing its durability. A wait latches the writer
    /// saturated, which is what stops new work from being admitted meanwhile.
    pub(crate) async fn send(&self, op: WriteOp) -> Result<()> {
        let tx = self.route(&op)?;
        let op = match tx.try_send(op) {
            Ok(()) => {
                self.backlog.record_accepted();
                return Ok(());
            }
            Err(mpsc::error::TrySendError::Closed(_)) => return Err(Self::closed()),
            Err(mpsc::error::TrySendError::Full(op)) => {
                self.backlog.latch();
                op
            }
        };
        // The room is taken before the op is counted, so a producer cancelled
        // while it waits leaves no op behind in the backlog either.
        let permit = tx.reserve().await.map_err(|_| Self::closed())?;
        self.backlog.record_accepted();
        permit.send(op);
        Ok(())
    }

    /// The sender of the shard `op` belongs to: a process always hashes to the
    /// same shard (the mapping the scheduler lanes and the event gate use), so
    /// its ops stay FIFO. The barrier hashes to shard 0, but `flush` is its
    /// only producer and queues it on every shard.
    fn route(&self, op: &WriteOp) -> Result<mpsc::Sender<WriteOp>> {
        let senders = self.senders.lock();
        let senders = senders.as_ref().ok_or_else(Self::closed)?;
        let shard = op.pid().map_or(0, |pid| pid_lane(pid, senders.len()));
        Ok(senders[shard].clone())
    }

    fn closed() -> ActError {
        ActError::Runtime("store writer channel closed".to_string())
    }

    pub(crate) fn depth(&self) -> usize {
        self.backlog.depth.load(Ordering::Acquire)
    }

    pub(crate) fn high_watermark(&self) -> usize {
        self.backlog.high_watermark.load(Ordering::Acquire)
    }

    /// Whether the write path is refusing new work right now — the store is
    /// not keeping up with its writes.
    pub(crate) fn saturated(&self) -> bool {
        self.backlog.is_saturated()
    }

    /// Block until all previously enqueued writes have been applied: a barrier
    /// is queued behind the backlog of every shard, and every one of them must
    /// ack, so the flush covers all shards rather than the one the caller's pid
    /// would hash to.
    ///
    /// Returns the first failure of a write enqueued since the previous
    /// flush: a flush only acks `Ok` when every write queued before the
    /// barrier was applied successfully, so callers can rely on the data
    /// being durable.
    pub(crate) async fn flush(&self) -> Result<()> {
        let senders = self.senders().ok_or_else(Self::closed)?;
        let mut acks = Vec::with_capacity(senders.len());
        for sender in &senders {
            let (tx, rx) = oneshot::channel();
            // Room first, then the count — a cancelled flush must not leave a
            // barrier in the backlog it never queued.
            let permit = sender.reserve().await.map_err(|_| Self::closed())?;
            self.backlog.record_accepted();
            permit.send(WriteOp::Barrier(tx));
            acks.push(rx);
        }
        let mut failed: Option<ActError> = None;
        for ack in acks {
            match ack.await {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    if failed.is_none() {
                        failed = Some(err);
                    }
                }
                Err(_) => {
                    if failed.is_none() {
                        failed = Some(ActError::Runtime("store writer task dropped".to_string()));
                    }
                }
            }
        }
        match failed {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    /// Snapshot of the shard senders, in shard order.
    fn senders(&self) -> Option<Vec<mpsc::Sender<WriteOp>>> {
        self.senders.lock().clone()
    }

    async fn apply(store: &Store, op: WriteOp) -> Result<()> {
        match op {
            WriteOp::Task(task) => Self::apply_task(store, &task).await,
            WriteOp::CloseDeliveries { pid, tid } => {
                store.close_deliveries(&pid, &tid).await?;
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
        // removed is dead data. Removal is queued on the writer too (FIFO
        // within the process's shard), so every write enqueued before the
        // removal has already been applied by now; skipping the late write
        // keeps it from re-creating rows or failing (missing procs row) for a
        // process that no longer exists.
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
        //     completed by the engine itself, with no client action ever) —
        //     except `Error` rows, which stay open for manual handling;
        //  2. re-check the removable mark — a process with no delivery rows
        //     (or all settled) is marked here, so the sweeper deletes it.
        if task.state().is_completed() {
            store.close_deliveries(&task.pid, &task.id).await?;
            let _ = store.try_mark_removable(&task.pid).await;
        }
        Ok(())
    }

    /// Flush every pending write, stop the writer consumers and wait until
    /// they have all fully exited. When this returns no writer task is left
    /// running: every op enqueued before the writer stopped has been applied.
    /// Later `send`/`flush` calls fail with a channel-closed error, and calling
    /// `close` again is a no-op.
    pub(crate) async fn close(&self) {
        // Failures of the drained writes were already logged by the writer
        // tasks; do not let them abort the shutdown.
        let _ = self.flush().await;
        self.senders.lock().take();
        // Taking the senders dropped every sender, so each consumer finishes
        // its backlog and exits on the closed channel.
        let tasks = std::mem::take(&mut *self.tasks.lock());
        for task in tasks {
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

    /// Shards every test writer is spawned with.
    const SHARDS: usize = 4;
    /// Ops each test writer queue holds.
    const CAPACITY: usize = 64;

    /// Memory kv that can be switched, from the test thread, to fail every
    /// `put` (a store outage), to block every `put` (holding the writer
    /// task inside an in-flight write), or to hold every `put` until
    /// [`TestKv::arm_join`] of them are inside one at the same time.
    struct TestKv {
        inner: MemoryStore,
        fail_put: AtomicBool,
        gate: AtomicBool,
        in_gate: AtomicBool,
        /// Number of concurrent `put`s the join gate opens at; 0 is off.
        join: AtomicUsize,
        in_flight: AtomicUsize,
        /// Set once the join gate latched open (see [`TestKv::join_gate`]).
        open: AtomicBool,
        peak: AtomicUsize,
    }

    /// Guard of [`TestKv::join_gate`]: the write stays counted as in flight
    /// for as long as it is inside the store.
    struct Gate<'a>(&'a AtomicUsize);

    impl Drop for Gate<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    impl TestKv {
        fn new() -> Self {
            Self {
                inner: MemoryStore::new(),
                fail_put: AtomicBool::new(false),
                gate: AtomicBool::new(false),
                in_gate: AtomicBool::new(false),
                join: AtomicUsize::new(0),
                in_flight: AtomicUsize::new(0),
                open: AtomicBool::new(false),
                peak: AtomicUsize::new(0),
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

        /// Let a `put` through only once `writes` of them have been inside the
        /// store at the same time; the gate then latches open, so no write
        /// ever waits for one that already finished. A writer that applies its
        /// ops one after another never reaches the latch, so its caller times
        /// out instead of the two writes ever overlapping.
        fn arm_join(&self, writes: usize) {
            self.in_flight.store(0, Ordering::SeqCst);
            self.open.store(false, Ordering::SeqCst);
            self.peak.store(0, Ordering::SeqCst);
            self.join.store(writes, Ordering::SeqCst);
        }

        /// Most `put`s observed inside the store at the same time.
        fn peak(&self) -> usize {
            self.peak.load(Ordering::SeqCst)
        }

        /// Yield until the writer task is parked inside a gated `put`.
        async fn wait_entered(&self) {
            while !self.in_gate.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        }

        async fn join_gate(&self) -> Option<Gate<'_>> {
            if self.join.load(Ordering::SeqCst) == 0 {
                return None;
            }
            let inside = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(inside, Ordering::SeqCst);
            while !self.open.load(Ordering::SeqCst) {
                if self.in_flight.load(Ordering::SeqCst) >= self.join.load(Ordering::SeqCst) {
                    self.open.store(true, Ordering::SeqCst);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            Some(Gate(&self.in_flight))
        }
    }

    #[async_trait::async_trait]
    impl KvStore for TestKv {
        async fn one(&self, key: &str) -> Result<Option<Vec<u8>>> {
            self.inner.one(key).await
        }

        async fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
            let _gate = self.join_gate().await;
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
        let writer = StoreWriter::spawn(store.clone(), SHARDS, CAPACITY);
        (store, kv, writer)
    }

    /// Two pids the writer routes to different shards, so their writes can be
    /// in flight at the same time.
    fn pids_on_distinct_shards(shards: usize) -> (String, String) {
        let a = "pids-a".to_string();
        let lane = pid_lane(&a, shards);
        let b = (0..shards)
            .map(|i| format!("pids-b{i}"))
            .find(|b| pid_lane(b, shards) != lane)
            .expect("a pool of more shards than one pid cannot use has a free shard");
        (a, b)
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
    /// finish, and joins the writer tasks, so nothing is left running when it
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
        // and the writer tasks were joined: nothing is left running
        assert!(
            writer.tasks.lock().is_empty(),
            "writer tasks were not joined by close()"
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

    /// `RemoveProc` is applied after the writes queued before it, then
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
        let writer = StoreWriter::spawn(store.clone(), SHARDS, CAPACITY);

        kv.arm_gate();
        enqueue(&writer, "p1").await;
        kv.wait_entered().await;
        assert_eq!(writer.depth(), 0); // the first op is in flight, not queued

        // same pid: the second op is queued behind the in-flight one
        enqueue(&writer, "p1").await;
        assert_eq!(writer.depth(), 1);

        kv.disarm_gate();
        writer.flush().await.unwrap();
        assert_eq!(writer.depth(), 0);
        assert!(writer.high_watermark() >= 1);
    }

    /// The per-shard queue is bounded, and the two producer classes react
    /// differently to no room: admitting new work is refused (so a backed-up
    /// store is not fed more work), while the bookkeeping of work already in
    /// flight waits for room instead of being dropped.
    #[tokio::test]
    async fn a_full_shard_refuses_new_work_and_stalls_bookkeeping() {
        let kv = Arc::new(TestKv::new());
        let store = Arc::new(Store::new(kv.clone()));
        // one shard with one op of room, so the bound is observable
        let writer = StoreWriter::spawn(store.clone(), 1, 1);
        let op = |tid: &str| WriteOp::EnqueueNext {
            pid: "p1".to_string(),
            tid: tid.to_string(),
        };

        // park the consumer inside a write: the op it took is off the queue,
        // leaving the single slot free again
        kv.arm_gate();
        enqueue(&writer, "p1").await;
        kv.wait_entered().await;
        writer.try_send(op("t2")).unwrap();

        // no room: the next new work is refused, not queued and not awaited
        let err = writer.try_send(op("t3")).unwrap_err();
        assert!(matches!(err, ActError::QueueFull), "{err}");
        assert!(writer.saturated(), "a refusal must latch the writer");

        // bookkeeping waits for room rather than being refused
        let stalled = tokio::time::timeout(Duration::from_millis(200), writer.send(op("t3"))).await;
        assert!(
            stalled.is_err(),
            "bookkeeping must wait for room while the shard is full"
        );

        kv.disarm_gate();
        writer.flush().await.unwrap();
        assert_eq!(writer.depth(), 0);
    }

    /// The saturation latch is shared: while one shard is out of room, new work
    /// for *any* process is refused — even one whose own shard is idle — and
    /// the writer re-opens once the backlog drains.
    #[tokio::test]
    async fn saturation_refuses_new_work_even_on_an_idle_shard() {
        let kv = Arc::new(TestKv::new());
        let store = Arc::new(Store::new(kv.clone()));
        // two shards with one op of room each
        let writer = StoreWriter::spawn(store.clone(), 2, 2);
        let (busy, idle) = pids_on_distinct_shards(2);
        let op = |pid: &str, tid: &str| WriteOp::EnqueueNext {
            pid: pid.to_string(),
            tid: tid.to_string(),
        };

        // fill the busy shard: its consumer is parked inside a write, so the op
        // it took left the queue and the one queued behind it is the last slot
        kv.arm_gate();
        enqueue(&writer, &busy).await;
        kv.wait_entered().await;
        enqueue(&writer, &busy).await;

        let err = writer.try_send(op(&busy, "t2")).unwrap_err();
        assert!(matches!(err, ActError::QueueFull), "{err}");
        assert!(writer.saturated(), "a refusal must latch the writer");

        // the idle shard has room, but new work is refused while latched
        let err = writer.try_send(op(&idle, "t1")).unwrap_err();
        assert!(matches!(err, ActError::QueueFull), "{err}");

        // once the backlog drains the latch releases and new work is admitted
        kv.disarm_gate();
        writer.flush().await.unwrap();
        assert!(!writer.saturated(), "a drained backlog re-opens the writer");
        writer.try_send(op(&idle, "t1")).unwrap();
        writer.flush().await.unwrap();
        assert!(durable(&store, &idle).await);
    }

    /// The write path is sharded by pid: ops of independent pids are applied
    /// concurrently — two of them sit inside the store at the same time — while
    /// every op of one pid is applied by one shard in enqueue order, which is
    /// what the deferred removal of a process relies on.
    #[tokio::test]
    async fn independent_pids_overlap_while_one_pid_keeps_its_order() {
        let (store, kv, writer) = test_writer();
        let (a, b) = pids_on_distinct_shards(SHARDS);

        // one op per pid: a serialized write path would park the first write
        // forever, since the second never reaches the store
        kv.arm_join(2);
        enqueue(&writer, &a).await;
        enqueue(&writer, &b).await;
        tokio::time::timeout(Duration::from_secs(5), writer.flush())
            .await
            .expect("ops of independent pids must be applied concurrently")
            .unwrap();
        assert!(
            kv.peak() >= 2,
            "the two writes must overlap in the store, peak={}",
            kv.peak()
        );

        // one pid: the removal is applied after the enqueue it follows, so the
        // row is gone — a reordered or concurrent pair would leave it behind,
        // and the pass-through gate above keeps both writes unblocked
        enqueue(&writer, &a).await;
        writer
            .send(WriteOp::RemoveProc { pid: a.clone() })
            .await
            .unwrap();
        writer.flush().await.unwrap();
        assert!(!durable(&store, &a).await, "one pid's ops must stay FIFO");
    }

    /// The deferred delivery close queued for a finished task settles the
    /// engine-owned rows and leaves an `Error` row for manual handling.
    #[tokio::test]
    async fn delivery_close_preserves_error_rows() {
        use crate::store::data::{Delivery, DeliveryStatus};

        let (store, _, writer) = test_writer();
        for (id, status) in [
            ("d-error", DeliveryStatus::Error),
            ("d-delivered", DeliveryStatus::Delivered),
            ("d-acked", DeliveryStatus::Acked),
        ] {
            store
                .deliveries()
                .create(&Delivery {
                    id: id.to_string(),
                    pid: "p1".to_string(),
                    tid: "t1".to_string(),
                    status,
                    ..Default::default()
                })
                .await
                .unwrap();
        }

        writer
            .send(WriteOp::CloseDeliveries {
                pid: "p1".to_string(),
                tid: "t1".to_string(),
            })
            .await
            .unwrap();
        writer.flush().await.unwrap();

        assert_eq!(
            store.deliveries().find("d-error").await.unwrap().status,
            DeliveryStatus::Error
        );
        assert_eq!(
            store.deliveries().find("d-delivered").await.unwrap().status,
            DeliveryStatus::Completed
        );
        assert_eq!(
            store.deliveries().find("d-acked").await.unwrap().status,
            DeliveryStatus::Completed
        );
    }
}
