//! The read model's write-behind path.
//!
//! Two writes must not happen inline in the engine's operation loop: a delivery row
//! (created on the channel path, which must stay reactive) and a process
//! removal (the sweeper). Both are ordered per pid — a removal must apply
//! after every write still queued for that process — so they go through one
//! sharded, bounded queue whose ops of one pid are applied in enqueue order
//! while independent processes are applied concurrently.
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use parking_lot::Mutex;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::{ActError, Result, store::Store, utils::LaneRouter};

pub(crate) enum WriteOp {
    /// Drop a process and its rows (tasks, outbox ops, message/delivery rows).
    /// Queued on the writer after any pending writes of the process, so
    /// removal can never race them: the completion markers apply first, then
    /// the rows are dropped.
    RemoveProc {
        pid: String,
    },
    /// Store the canonical message row and one ack-channel delivery row of a
    /// message (see [`Store::store_message_delivery`]).
    ///
    /// The row is written on the process's own shard because it is created
    /// off the task's writes: the close of a finished task settles every
    /// delivery of that task, and the sweeper deletes a finished process's
    /// rows (after any still-queued write). A delivery created outside that
    /// order can land after either — left open on a process whose rows are
    /// already gone (nothing would ever settle it, the sweeper needs every
    /// delivery settled), or re-created behind the removal. On the shard the
    /// three are sequential: the close sees the row, or the row sees a task
    /// that is already closed (and is born settled), or the removal came first
    /// (and it is dropped).
    ///
    /// The rows are boxed: every shard's bounded queue is pre-allocated at
    /// op-size slots, and a message plus a delivery inline would multiply that
    /// buffer by their ~500 bytes for an op that is one row pair.
    StoreDelivery {
        message: Box<crate::data::Message>,
        delivery: Box<crate::data::Delivery>,
        /// `Ok(true)` stored; `Ok(false)` the process was already gone and
        /// nothing was stored; `Err` the store failed.
        reply: oneshot::Sender<Result<bool>>,
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
            WriteOp::RemoveProc { pid } => Some(pid.as_str()),
            // the message's process is the shard its row must follow: the
            // task writes and the removal of that process
            WriteOp::StoreDelivery { delivery, .. } => Some(delivery.pid.as_str()),
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
        #[allow(deprecated)]
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
/// Queues are bounded and nothing is ever buffered without limit:
///
/// - [`Self::send`] queues the bookkeeping of work already admitted (delivery
///   rows and process removals). These ops decide rows the engine has already
///   committed to, so they are never refused — they wait for room, stalling the
///   process they belong to rather than losing its durability. A wait latches
///   the writer saturated, which is what keeps producers from piling more work
///   behind a store that is already behind.
/// - [`Self::flush`] queues a barrier on every shard and waits for room, so a
///   flush always completes once the backlog drains.
///
/// The saturation latch is shared: a wait for room latches it, and it releases
/// once the backlog is back under half the capacity.
#[derive(Clone)]
pub(crate) struct StoreWriter {
    /// One sender per shard, indexed by the writer's [`LaneRouter`]. `None`
    /// once `close` took them, which is what makes later `send`/`flush` calls
    /// fail.
    ///
    /// Each queue is bounded by `capacity / shards` ops (at least one).
    senders: Arc<Mutex<Option<Vec<mpsc::Sender<WriteOp>>>>>,
    /// One consumer per shard; `close` joins them all.
    tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
    /// The pid → shard mapping. Owned by the writer (its own router instance,
    /// sized to the shard count) so shard skew stays balanced independently of
    /// the scheduler lanes.
    router: Arc<LaneRouter>,
    backlog: Arc<Backlog>,
}

impl StoreWriter {
    /// Spawn `shards` consumers (at least one), each with its own queue bounded
    /// by `capacity / shards` ops (at least one). They run on the ambient
    /// tokio runtime. Routing goes through `router`: pass the writer's own
    /// router to keep its assignments stable and balanced.
    pub(crate) fn spawn(
        store: Arc<Store>,
        shards: usize,
        capacity: usize,
        router: Arc<LaneRouter>,
    ) -> Self {
        let shards = shards.max(1);
        let lane_capacity = (capacity.max(1) / shards).max(1);
        let backlog = Arc::new(Backlog::new(lane_capacity * shards));
        let mut senders = Vec::with_capacity(shards);
        let mut tasks = Vec::with_capacity(shards);
        for _ in 0..shards {
            let (tx, mut rx) = mpsc::channel::<WriteOp>(lane_capacity);
            let store = store.clone();
            let backlog = backlog.clone();
            let router = router.clone();
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
                        op => Self::apply(&store, &router, op).await,
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
            router,
            backlog,
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

    /// The sender of the shard `op` belongs to: a process always maps to the
    /// same shard (the writer's [`LaneRouter`]), so its ops stay FIFO. The
    /// barrier maps to shard 0, but `flush` is its only producer and queues it
    /// on every shard.
    fn route(&self, op: &WriteOp) -> Result<mpsc::Sender<WriteOp>> {
        let senders = self.senders.lock();
        let senders = senders.as_ref().ok_or_else(Self::closed)?;
        let shard = op.pid().map_or(0, |pid| self.router.lane_of(pid));
        Ok(senders[shard].clone())
    }

    fn closed() -> ActError {
        ActError::Shutdown
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
    /// would hash to. A pid-scoped reader needs only [`Self::flush_pid`].
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

    /// Block until every write previously queued for `pid` has been applied:
    /// a barrier on the one shard the pid maps to, where all of the pid's
    /// ops are FIFO. A pid-scoped read — a cache-miss load of the process —
    /// needs exactly this visibility (never a row older than the pid's
    /// pending writes) without paying [`Self::flush`]'s wait for the backlog
    /// of every other shard.
    ///
    /// Fails like [`Self::flush`]: the writer is closed, or a write queued
    /// before the barrier on that shard failed.
    pub(crate) async fn flush_pid(&self, pid: &str) -> Result<()> {
        let senders = self.senders().ok_or_else(Self::closed)?;
        let (tx, rx) = oneshot::channel();
        // Room first, then the count — a cancelled barrier must not stay in a
        // backlog it never queued (same discipline as `flush`).
        let permit = senders[self.router.lane_of(pid)]
            .reserve()
            .await
            .map_err(|_| Self::closed())?;
        self.backlog.record_accepted();
        permit.send(WriteOp::Barrier(tx));
        match rx.await {
            Ok(result) => result,
            Err(_) => Err(ActError::Runtime("store writer task dropped".to_string())),
        }
    }

    /// Snapshot of the shard senders, in shard order.
    fn senders(&self) -> Option<Vec<mpsc::Sender<WriteOp>>> {
        self.senders.lock().clone()
    }

    async fn apply(store: &Store, router: &LaneRouter, op: WriteOp) -> Result<()> {
        match op {
            WriteOp::RemoveProc { pid } => {
                store.remove_proc(&pid).await?;
                // The removal is durable: release the pid's shard assignment.
                // Any late write for it is a dead write and may re-register on
                // the next shard in turn.
                router.forget(&pid);
                Ok(())
            }
            WriteOp::StoreDelivery {
                message,
                delivery,
                reply,
            } => {
                let stored = store.store_message_delivery(&message, &delivery).await;
                // the caller (the channel delivery path) decides on the
                // outcome; the failure travels back to it and stays a shard
                // failure for the next barrier
                let _ = reply.send(match &stored {
                    Ok(stored) => Ok(*stored),
                    Err(err) => Err(err.clone()),
                });
                stored.map(|_| ())
            }
            // Acked by the writer loop before `apply`, never reached here.
            WriteOp::Barrier(_) => unreachable!("barrier is acked by the writer loop"),
        }
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

/// Store one message's canonical row and one ack-channel delivery row through
/// the write-behind queue, written on the process's shard so the create cannot
/// land after the close (or the removal) that decides the row's fate — see
/// [`WriteOp::StoreDelivery`]. `Ok(false)` means nothing was stored: the
/// process is gone, so the message is delivered without a delivery row instead
/// of re-creating rows behind its removal.
pub(crate) async fn store_delivery(
    writer: &StoreWriter,
    message: &crate::data::Message,
    delivery: &crate::data::Delivery,
) -> Result<bool> {
    let (tx, rx) = oneshot::channel();
    writer
        .send(WriteOp::StoreDelivery {
            message: Box::new(message.clone()),
            delivery: Box::new(delivery.clone()),
            reply: tx,
        })
        .await?;
    rx.await
        .map_err(|_| ActError::Runtime("store writer dropped the delivery".to_string()))?
}

/// Evict one process's rows without waiting for them: `RemoveProc` is
/// serialized through the writer (FIFO), so it can never race writes still
/// queued for the process — its completion markers are applied first, then the
/// rows are dropped once the shard reaches the op. The terminal cleanup holds
/// the pid's lifecycle lock and flushes the pid shard before releasing it, so
/// a same-pid start cannot interleave with the removal.
pub(crate) async fn queue_removal(store: &Store, writer: &StoreWriter, pid: &str) -> Result<()> {
    debug!("remove pid={pid}");
    // Read before the writer drops the row: the directory lives in the
    // process's env, and a swept process's instance is already gone.
    let workdir = store.proc_workdir(pid).await;
    // The directory is created by the start and holds nothing the rows depend
    // on, so it goes with them — and goes FIRST: a crash between the two leaves
    // the row still marked, which the next sweep converges on, while the
    // reverse order would strand the directory with no row left to find it.
    remove_workdir(pid, workdir);
    writer
        .send(WriteOp::RemoveProc {
            pid: pid.to_string(),
        })
        .await
}

/// Delete the directory a process's filesystem access was confined to —
/// `<workdir>/<pid>`, as carried in the process's env — when the process named
/// `pid` has one.
///
/// The shape is checked: only a directory whose last component is the pid
/// itself is removed, the only shape a start ever creates, so a row that names
/// something else cannot delete it. Removal is best-effort — a directory holds
/// no engine state, so a failure is reported and never blocks the removal it
/// belongs to (pinning a dead process's rows, and its pid, on a filesystem
/// error would be the worse outcome).
fn remove_workdir(pid: &str, dir: Option<std::path::PathBuf>) {
    let Some(dir) = dir else {
        return;
    };
    if dir.file_name().and_then(|name| name.to_str()) != Some(pid) {
        warn!(
            pid = %pid, dir = %dir.display(),
            "not removing a workdir that is not named after its process"
        );
        return;
    }
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => debug!(pid = %pid, "removed the process workdir"),
        // Already gone (an interrupted removal that was retried, or an
        // operator cleaning up): nothing to do.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            warn!(
                pid = %pid, dir = %dir.display(), error = %err,
                "failed to remove the process workdir"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{KvStore, MemoryStore, ScanOptions};
    use crate::utils::shared;
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
        /// When set, only the `put`s whose key carries this substring (a pid)
        /// park on the gate: holds one shard's write in flight while every
        /// other shard keeps applying.
        gate_key: Mutex<Option<String>>,
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
                gate_key: Mutex::new(None),
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

        /// Park only the `put`s whose key carries this pid (a collection row's
        /// index keys embed the pid): that pid's shard writer stalls inside
        /// the store while every other shard keeps working.
        fn gate_pid(&self, pid: &str) {
            *self.gate_key.lock() = Some(pid.to_string());
        }

        fn disarm_pid_gate(&self) {
            *self.gate_key.lock() = None;
        }

        fn key_gated(&self, key: &str) -> bool {
            self.gate_key
                .lock()
                .as_deref()
                .map(|needle| key.contains(needle))
                .unwrap_or(false)
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
            if self.gate.load(Ordering::SeqCst) || self.key_gated(key) {
                self.in_gate.store(true, Ordering::SeqCst);
                while self.gate.load(Ordering::SeqCst) || self.key_gated(key) {
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
        let writer = StoreWriter::spawn(store.clone(), SHARDS, CAPACITY, shared(SHARDS));
        (store, kv, writer)
    }

    /// Two pids a fresh `shards`-shard writer assigns to distinct shards: the
    /// round-robin router gives the first two registered pids shards 0 and 1,
    /// so the callers must enqueue these before any other pid.
    fn pids_on_distinct_shards(shards: usize) -> (String, String) {
        assert!(shards > 1, "distinct shards need more than one shard");
        ("pids-a".to_string(), "pids-b".to_string())
    }

    /// The canonical-row + delivery-row pair the channel path writes — the
    /// write the writer still applies besides a removal, so it is what every
    /// test here drives the shards with. `tid` stays empty: a delivery that
    /// names a task is dropped unless that task's process row exists, and the
    /// tests here are about the write path, not the read model.
    fn delivery(pid: &str, mid: &str) -> WriteOp {
        let (reply, _rx) = oneshot::channel();
        WriteOp::StoreDelivery {
            message: Box::new(crate::store::data::Message {
                id: mid.to_string(),
                pid: pid.to_string(),
                ..Default::default()
            }),
            delivery: Box::new(crate::store::data::Delivery {
                id: format!("delivery-{mid}"),
                msg_id: mid.to_string(),
                pid: pid.to_string(),
                ..Default::default()
            }),
            reply,
        }
    }

    async fn enqueue(writer: &StoreWriter, pid: &str) {
        writer.send(delivery(pid, &message_id(pid))).await.unwrap();
    }

    fn message_id(pid: &str) -> String {
        format!("msg-{pid}")
    }

    async fn durable(store: &Store, pid: &str) -> bool {
        store.messages().find(&message_id(pid)).await.is_ok()
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

    /// `flush_pid` barriers the pid's own shard only: it acks while another
    /// shard is stuck mid-write — where the global `flush` waits for it.
    #[tokio::test]
    async fn flush_pid_waits_only_for_its_own_shard() {
        let (store, kv, writer) = test_writer();
        let (a, b) = pids_on_distinct_shards(SHARDS);

        // park only the writes of `b`: its shard's writer task stalls inside
        // the store while `a`'s shard keeps applying. Index keys encode the
        // pid value with `-` escaped (`pids-b2` lands as `pids=2Db2`), so the
        // gate must match the escaped form.
        kv.gate_pid(&b.replace('-', "=2D"));
        enqueue(&writer, &b).await;
        kv.wait_entered().await;

        // a write for `a` lands, and its shard barrier acks, while `b` stalls
        enqueue(&writer, &a).await;
        tokio::time::timeout(Duration::from_secs(5), writer.flush_pid(&a))
            .await
            .expect("flush_pid must not wait for another shard's backlog")
            .unwrap();
        assert!(durable(&store, &a).await);

        // the global flush covers every shard: it cannot complete while `b`
        // is stuck
        assert!(
            tokio::time::timeout(Duration::from_millis(200), writer.flush())
                .await
                .is_err(),
            "flush must wait for the stalled shard"
        );

        kv.disarm_pid_gate();
        writer.flush().await.unwrap();
        assert!(durable(&store, &b).await);
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

    /// After `close` the writer is gone: further sends fail with the
    /// structured shutdown error, and `close` is idempotent.
    #[tokio::test]
    async fn send_and_flush_fail_after_close() {
        let (store, _, writer) = test_writer();
        enqueue(&writer, "p1").await;
        writer.close().await;
        assert!(durable(&store, "p1").await);

        writer.close().await; // no-op

        let send_err = writer.send(delivery("p2", "msg-p2")).await.unwrap_err();
        assert!(
            matches!(send_err, ActError::Shutdown),
            "a post-close send must report the structured shutdown error: {send_err}"
        );
        let flush_err = writer.flush().await.unwrap_err();
        assert!(
            matches!(flush_err, ActError::Shutdown),
            "a post-close flush must report the structured shutdown error: {flush_err}"
        );
    }

    /// `RemoveProc` is applied after the writes queued before it, then
    /// deletes the process's rows; one flush covers both and reports no
    /// failure. Removing an absent process is a no-op.
    #[tokio::test]
    async fn remove_proc_drops_rows_after_pending_writes() {
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
        let writer = StoreWriter::spawn(store.clone(), SHARDS, CAPACITY, shared(SHARDS));

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

    /// A full shard makes the bookkeeping wait for room (never drop it) and
    /// latches the writer saturated, so producers can see that the store is
    /// behind instead of piling more work onto it.
    #[tokio::test]
    async fn a_full_shard_stalls_bookkeeping_and_latches_saturation() {
        let kv = Arc::new(TestKv::new());
        let store = Arc::new(Store::new(kv.clone()));
        // one shard with one op of room, so the bound is observable
        let writer = StoreWriter::spawn(store.clone(), 1, 1, shared(1));

        // park the consumer inside a write: the op it took is off the queue,
        // leaving the single slot free again
        kv.arm_gate();
        enqueue(&writer, "p1").await;
        kv.wait_entered().await;
        enqueue(&writer, "p1").await;
        assert_eq!(writer.depth(), 1, "the second op fills the shard");

        // no room: the next bookkeeping write waits for room instead of being
        // dropped, and the wait latches the writer
        let stalled = tokio::time::timeout(
            Duration::from_millis(200),
            writer.send(delivery("p1", "m3")),
        )
        .await;
        assert!(
            stalled.is_err(),
            "bookkeeping must wait for room while the shard is full"
        );
        assert!(writer.saturated(), "a wait for room must latch the writer");

        kv.disarm_gate();
        writer.flush().await.unwrap();
        assert_eq!(writer.depth(), 0);
        assert!(!writer.saturated(), "a drained backlog re-opens the writer");
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
}
