use super::writer::{StoreWriter, WriteOp};
use crate::{
    ActError, Action, Config, Result,
    query::{Expr, Filter, Query},
    scheduler::{Process, Runtime, Task, TaskState},
    store::{KvStore, MemoryStore, Store, query::Sort},
    utils::consts,
};
use parking_lot::{Mutex, RwLock};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::Arc,
};
use tracing::{debug, instrument, warn};

/// Result shared by all callers waiting for one `proc` load.
type LoadedProc = Result<Option<Arc<Process>>>;

/// A per-pid load shared with concurrent callers. The watch channel starts as
/// `None` and is set exactly once when the leader finishes (or is cancelled).
struct InflightProc {
    rx: tokio::sync::watch::Receiver<Option<LoadedProc>>,
}

/// Removes an in-flight slot and wakes waiters if its owner is cancelled.
struct InflightGuard {
    pid: String,
    inflight: Arc<InflightProc>,
    tx: tokio::sync::watch::Sender<Option<LoadedProc>>,
    loading: Arc<RwLock<HashMap<String, Arc<InflightProc>>>>,
}

impl InflightGuard {
    fn finish(mut self, result: LoadedProc) {
        self.tx.send_replace(Some(result));
        self.remove();
    }

    fn remove(&mut self) {
        let mut loading = self.loading.write();
        if loading
            .get(&self.pid)
            .is_some_and(|current| Arc::ptr_eq(current, &self.inflight))
        {
            loading.remove(&self.pid);
        }
    }
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        let mut loading = self.loading.write();
        let is_owner = loading
            .get(&self.pid)
            .is_some_and(|current| Arc::ptr_eq(current, &self.inflight));
        if is_owner {
            loading.remove(&self.pid);
            self.tx.send_replace(Some(Err(ActError::Runtime(
                "process load cancelled".to_string(),
            ))));
        }
    }
}

/// Resident-slot commitments of in-flight store loads. [`Cache::admit`]
/// decides capacity from the resident map alone, so this counter only makes a
/// restore pass's free-slot claim visible to other passes: while a load is
/// reading rows, `resident + reserved` is the capacity they must treat as
/// taken, so they skip rows already being loaded instead of decoding them
/// again.
#[derive(Default)]
struct Admission {
    reserved: usize,
}

/// Slots on the resident set committed to one in-flight store load. A pass
/// reserves every free slot before its store I/O (see `Cache::reserve_slots`)
/// and releases them as loaded rows become resident; whatever is left is
/// released on drop, covering short loads, errors and early returns. Two
/// concurrent passes therefore never load — and race to start — the same
/// parked row (each `start` is guarded per `Process` instance, not per pid, so
/// a duplicate would run the workflow twice).
struct SlotReservation {
    cache: Cache,
    remaining: usize,
}

impl SlotReservation {
    /// Slots still available to this pass.
    fn remaining(&self) -> usize {
        self.remaining
    }

    /// Make `proc` resident, consuming one reserved slot. Returns `false` —
    /// without consuming — when it is already resident or `cap` was reached
    /// meanwhile: admission ignores reservations, so it may have taken the
    /// slot while the row was being read. The caller must then NOT start the
    /// process.
    fn commit(&mut self, proc: &Arc<Process>) -> bool {
        if self.remaining == 0 {
            return false;
        }
        // `admission` before `procs`, the order every capacity path uses.
        let mut admission = self.cache.admission.lock();
        let mut procs = self.cache.procs.write();
        if procs.len() >= self.cache.cap || procs.contains_key(proc.id()) {
            return false;
        }
        procs.insert(proc.id().to_string(), proc.clone());
        admission.reserved -= 1;
        self.remaining -= 1;
        true
    }
}

impl Drop for SlotReservation {
    fn drop(&mut self) {
        if self.remaining > 0 {
            self.cache.admission.lock().reserved -= self.remaining;
        }
    }
}

/// Deduplicated FIFO of in-flight pids awaiting a free resident slot after a
/// boot-resume overflow. `ids` mirrors `queue`: `push_back` is a no-op for a
/// pid already queued, so repeated overflow scans cannot enqueue a duplicate
/// (each resume attempt would otherwise pay a store lookup and the queue
/// would grow with the number of scans, not the number of overflow rows).
#[derive(Default)]
struct PendingResume {
    queue: VecDeque<String>,
    ids: HashSet<String>,
}

impl PendingResume {
    fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Queue `pid` at the back unless it is already queued; returns whether
    /// the pid was newly queued.
    fn push_back(&mut self, pid: String) -> bool {
        if !self.ids.insert(pid.clone()) {
            return false;
        }
        self.queue.push_back(pid);
        true
    }

    fn pop_front(&mut self) -> Option<String> {
        let pid = self.queue.pop_front()?;
        self.ids.remove(&pid);
        Some(pid)
    }

    /// Return a just-popped `pid` to the front of the queue.
    fn push_front(&mut self, pid: String) {
        self.ids.insert(pid.clone());
        self.queue.push_front(pid);
    }
}

#[derive(Clone)]
pub struct Cache {
    cap: usize,
    /// Resident set: every process with a live in-memory instance, keyed by
    /// pid. There is NO eviction policy — a resident process may be running
    /// (it must stay reachable by pid for its whole life: reloading it from
    /// the store would create a second instance racing the first) or a
    /// finished process briefly waiting for its terminal event handler to
    /// evict it. Capacity is enforced by [`Self::admit`] at start time, not
    /// by cache pressure: processes beyond `cap` are parked in the store
    /// (`None` state, no task rows) and started by [`Self::start_parked`] when a
    /// terminal event frees a slot.
    procs: Arc<RwLock<HashMap<String, Arc<Process>>>>,
    /// In-flight store loads, keyed by pid. The first miss becomes the leader;
    /// every later caller waits for and reuses its loaded `Arc<Process>`.
    loading: Arc<RwLock<HashMap<String, Arc<InflightProc>>>>,
    /// Pids claimed by an in-process admission (resident or parked). Unlike
    /// durable rows, this check is synchronous and closes the start-time
    /// lookup/admit race. It is process-local: multi-instance deployments still
    /// need an external uniqueness guarantee for externally supplied pids.
    claimed: Arc<RwLock<HashSet<String>>>,
    /// Pids whose process rows the sweeper has already removed. The removal is
    /// queued on the writer (FIFO), but a close enqueued *before* the removal
    /// can still be applied after it — writing a row back under a process that
    /// no longer exists, which nothing would ever delete. The outbox close
    /// paths consult this set and skip that dead bookkeeping.
    removed: Arc<RwLock<HashSet<String>>>,
    /// Boot-resume overflow queue: pids of in-flight (`Ready`/`Running`/
    /// `Pending`) rows that did not fit the resident cap at boot, oldest
    /// first. [`Self::resume_from_queue`] drains them into free slots (the
    /// terminal-event restore) — without it those rows would wait forever,
    /// since [`Self::start_parked`] only refills parked `None` rows. Enqueue
    /// is deduplicated ([`PendingResume`]), so a repeated overflow scan never
    /// queues a pid twice.
    pending_resume: Arc<RwLock<PendingResume>>,
    store: Arc<Store>,
    writer: StoreWriter,
    /// Capacity bookkeeping: resident slots committed to loads that are still
    /// reading rows (see [`Admission`]). Terminal proc events of different
    /// processes run concurrently, so admission (`admit`) and whole restore
    /// passes share it — but only for the synchronous decision. Store I/O and
    /// `Process::start` happen outside every lock, so a slow store no longer
    /// serializes admission behind a restore pass (or vice versa).
    admission: Arc<Mutex<Admission>>,
}

impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cache")
            .field("cap", &self.cap())
            .field("count", &self.count())
            .finish()
    }
}

impl Cache {
    pub fn new(config: &Config, store: Option<Arc<dyn KvStore>>) -> crate::Result<Self> {
        let store = Arc::new(Store::new(
            store.unwrap_or_else(|| Arc::new(MemoryStore::new())),
        ));
        Ok(Self {
            cap: config.cache_cap() as usize,
            procs: Arc::new(RwLock::new(HashMap::new())),
            loading: Arc::new(RwLock::new(HashMap::new())),
            claimed: Arc::new(RwLock::new(HashSet::new())),
            removed: Arc::new(RwLock::new(HashSet::new())),
            pending_resume: Arc::new(RwLock::new(PendingResume::default())),
            store: store.clone(),
            writer: StoreWriter::spawn(
                store,
                config.store_writer_workers(),
                config.store_writer_queue_cap(),
            ),
            admission: Arc::new(Mutex::new(Admission::default())),
        })
    }

    pub fn store(&self) -> Arc<Store> {
        self.store.clone()
    }

    pub fn cap(&self) -> usize {
        self.cap
    }

    pub fn count(&self) -> usize {
        self.procs.read().len()
    }

    /// Current number of write operations accepted but not yet consumed.
    pub fn store_writer_depth(&self) -> usize {
        self.writer.depth()
    }

    pub fn store_writer_high_watermark(&self) -> usize {
        self.writer.high_watermark()
    }

    /// Whether the store write path is saturated — refusing new work until its
    /// backlog drains (see [`StoreWriter`]).
    pub fn store_writer_saturated(&self) -> bool {
        self.writer.saturated()
    }

    /// Snapshot of the boot-resume overflow queue (test visibility).
    #[cfg(test)]
    pub(crate) fn pending_resume_ids(&self) -> Vec<String> {
        self.pending_resume.read().queue.iter().cloned().collect()
    }

    pub async fn close(&self) {
        self.writer.close().await;
    }

    #[instrument(skip(self, proc), fields(pid = %proc.id()))]
    pub async fn push_proc(&self, proc: &Arc<Process>) -> Result<()> {
        self.push_proc_pri(proc, true).await?;

        Ok(())
    }

    pub fn procs(&self) -> Vec<Arc<Process>> {
        self.procs.read().values().cloned().collect()
    }

    /// The resident process for `pid`, if the engine holds it in memory. A
    /// process that is parked (`None`), queued for a later slot, or already
    /// evicted is not resident; its durable rows are the store's to answer for.
    pub(crate) fn resident(&self, pid: &str) -> Option<Arc<Process>> {
        self.get_proc(pid)
    }

    /// Slots currently committed to an in-flight load, plus the resident count
    /// — the two halves of the capacity decision (see [`Self::reserve_slots`]).
    /// Test visibility: a pass that leaked its reservation, or a resident set
    /// that never released a slot, is invisible from the rows alone.
    #[cfg(test)]
    pub(crate) fn capacity_state(&self) -> (usize, usize) {
        (self.procs.read().len(), self.admission.lock().reserved)
    }

    #[instrument(skip(self, rt), fields(pid = %pid))]
    pub async fn proc(&self, pid: &str, rt: &Arc<Runtime>) -> Result<Option<Arc<Process>>> {
        debug!("process: pid={pid}");
        match self.get_proc(pid) {
            Some(proc) => Ok(Some(proc.clone())),
            None => {
                // The load must not observe rows older than the writes still
                // queued for this pid. Every op of one pid is FIFO on one
                // shard, so a barrier on that shard gives the same
                // read-after-write visibility as a full `flush` — without
                // waiting for the backlog of every other shard.
                self.writer.flush_pid(pid).await?;
                // Coalesce misses. The insert/remove critical section is short;
                // only the leader performs store I/O, while waiters clone the
                // same process pointer from the watch result.
                // Check and claim leadership atomically; otherwise concurrent
                // callers can all observe an empty map before any insert and
                // start independent loads for the same pid.
                let (leader, wait_rx) = {
                    let mut loading = self.loading.write();
                    if let Some(inflight) = loading.get(pid) {
                        (None, Some(inflight.rx.clone()))
                    } else if let Some(proc) = self.get_proc(pid) {
                        // A leader we missed finished inside our `flush` and
                        // cached the process. Its insert precedes the removal
                        // of this in-flight entry (both under `loading`, and
                        // the insert comes first), so an absent entry means any
                        // earlier load is already visible: reusing it here
                        // avoids a second load returning a second instance for
                        // the same pid. `loading` is always taken before
                        // `procs`, never the other way around.
                        return Ok(Some(proc));
                    } else {
                        let (tx, rx) = tokio::sync::watch::channel(None);
                        let inflight = Arc::new(InflightProc { rx: rx.clone() });
                        loading.insert(pid.to_string(), inflight.clone());
                        let guard = InflightGuard {
                            pid: pid.to_string(),
                            inflight,
                            tx,
                            loading: self.loading.clone(),
                        };
                        (Some(guard), None)
                    }
                };
                if let Some(mut rx) = wait_rx {
                    while rx.borrow_and_update().is_none() {
                        rx.changed().await.map_err(|_| {
                            ActError::Runtime("process load leader was dropped".to_string())
                        })?;
                    }
                    let result = rx.borrow_and_update().clone().ok_or_else(|| {
                        ActError::Runtime("process load result missing".to_string())
                    })?;
                    return result;
                }

                let guard = leader.expect("process load leader must own its in-flight guard");
                let loaded = self.store.load_proc(pid, rt).await;
                if let Ok(Some(proc)) = &loaded {
                    debug!(pid = %pid, "loaded process");
                    // add to cache — unless it is parked: a durable `None`
                    // row that was never started (the resident set was full at
                    // start time). Caching it would occupy a slot forever —
                    // `restore` skips resident pids, so it would never start.
                    if !proc.state().is_none()
                        && self.count() < self.cap()
                        && let Err(err) = self.push_proc_pri(proc, false).await
                    {
                        guard.finish(Err(err.clone()));
                        return Err(err);
                    }
                }
                guard.finish(loaded.clone());
                loaded
            }
        }
    }

    #[instrument(skip(self), fields(pid = %pid))]
    pub async fn remove(&self, pid: &str) -> Result<bool> {
        self.queue_removal(pid).await?;
        // When this returns, the removal is durable and no pending write can
        // resurrect the rows afterwards.
        self.writer.flush().await?;
        Ok(true)
    }

    /// Evict the process from memory and queue its durable removal without
    /// waiting for it: `RemoveProc` is serialized through the writer (FIFO),
    /// so it can never race writes still queued for the process — its
    /// completion markers are applied first, then the rows are dropped once
    /// the shard reaches the op. Callers that need the removal to be durable
    /// when they return flush afterwards: [`Self::remove`] for one pid,
    /// [`Self::sweep_removable`] once for the whole batch.
    async fn queue_removal(&self, pid: &str) -> Result<()> {
        debug!("remove pid={pid}");
        // Read before the writer drops the row: the directory lives in the
        // process's env, and a swept process's instance is already evicted.
        let workdir = self.store.proc_workdir(pid).await;
        self.procs.write().remove(pid);
        self.claimed.write().remove(pid);
        // anything this process enqueues after this point is dead bookkeeping
        self.removed.write().insert(pid.to_string());
        // The directory is created by the start and holds nothing the rows
        // depend on, so it goes with them — and goes FIRST: a crash between
        // the two leaves the row still marked, which the next sweep converges
        // on, while the reverse order would strand the directory with no row
        // left to find it.
        remove_workdir(pid, workdir);
        self.writer
            .send(WriteOp::RemoveProc {
                pid: pid.to_string(),
            })
            .await
    }

    /// The sweeper: delete every finished process whose deliveries have all
    /// settled (see [`Store::sweep_settled_procs`]) and evict each from the
    /// in-memory cache. Runs on the retry-timer tick; deletion is never
    /// synchronous with the process completion — it waits for the deliveries
    /// to settle. Removal goes through the writer (`RemoveProc`, FIFO after
    /// any still-queued writes of the process), so it cannot race them.
    #[instrument(skip(self))]
    pub(crate) async fn sweep_removable(&self) -> Result<usize> {
        let pids = self.store.sweep_settled_procs(256).await?;
        for pid in &pids {
            self.queue_removal(pid).await?;
        }
        // One barrier covers the whole sweep: each `RemoveProc` is already
        // FIFO after its process's pending writes, so the batch flush only
        // bounds how long the dropped rows can linger for readers — it must
        // not pay one all-shard wait per process.
        if !pids.is_empty() {
            self.writer.flush().await?;
        }
        Ok(pids.len())
    }

    /// Evict a process from the in-memory cache only — its durable rows are
    /// left untouched. Called when a process finishes (its terminal proc
    /// event): the freed slot lets [`Self::start_parked`] start a parked process
    /// into it. The store rows themselves are removed later by the sweeper,
    /// once every delivery of the process's messages settled.
    pub(crate) fn evict(&self, pid: &str) {
        self.procs.write().remove(pid);
    }

    /// Roll back an admission whose [`Process::start`] failed: drop the
    /// in-memory instance and give the pid back. A start that errored never
    /// became a running process, and for a pid with no rows nothing else ever
    /// releases its claim — the sweeper only removes rows that exist — so a
    /// retained claim would reject every later start of that externally
    /// supplied pid with a misleading "duplicated in running process list".
    ///
    /// The directory the failed start created (`<acl workdir root>/<pid>`)
    /// goes the same way for the same reason: with no row, no sweep will ever
    /// find it.
    ///
    /// The claim is released only when the store confirms the pid has no row:
    /// a durable row occupies its pid for the row's whole lifetime and
    /// [`Self::remove`] (the sweeper) is what releases the claim then. A store
    /// that cannot answer keeps the claim too — a row that cannot be ruled out
    /// must never be handed to a second admission.
    pub(crate) async fn abandon(&self, proc: &Arc<Process>) {
        // the in-memory instance goes first: whatever failed inside
        // `Process::start`, this workflow is not running, so it must not hold
        // a resident slot nor answer `proc()` for a pid that is dead
        self.procs.write().remove(proc.id());
        match self.store.procs().exists(proc.id()).await {
            Ok(true) => {}
            Ok(false) => {
                // No row, so nothing will ever sweep the directory the start
                // created: it goes with the claim — and before the claim is
                // released, or a start that takes the freed pid could see the
                // directory it just created removed.
                remove_workdir(proc.id(), proc.workdir());
                self.claimed.write().remove(proc.id());
            }
            Err(err) => {
                warn!(
                    pid = %proc.id(), error = %err,
                    "cannot check for a durable row while rolling back a failed start: \
                     the pid keeps its claim"
                );
            }
        }
    }

    /// Capacity admission for a fresh start. Returns `true` when the process
    /// was admitted to the resident set and the caller should run it now;
    /// returns `false` when the set is full — the process is *parked*: its
    /// durable row (state `None`, no task rows) is persisted and it is NOT
    /// cached, and [`Self::start_parked`] starts it when a terminal event frees a
    /// slot. A running process is never evicted to make room — eviction
    /// would orphan the pid (its tasks/tick loop keep the old `Arc<Process>`
    /// alive), and a later reload would create a second instance racing it.
    ///
    /// Parked child processes of a running workflow simply wait for any free
    /// slot like everyone else; there is no lock-step dependency, so a full
    /// resident set delays them but never deadlocks.
    #[instrument(skip(self, proc), fields(pid = %proc.id()))]
    pub(crate) async fn admit(&self, proc: &Arc<Process>) -> Result<bool> {
        // Synchronous capacity decision: take a resident slot now, or park.
        // Only the parking write below touches the store, and it happens after
        // the decision is made, so a slow store cannot serialize this
        // admission behind a restore pass (or vice versa). Parking happens
        // only when the resident set is genuinely full, so a slot freed by a
        // terminal event can never leave a parked process stranded.
        let parked = {
            let mut procs = self.procs.write();
            // Claim the pid before any store I/O. Both resident and parked
            // admissions leave this marker installed, so a second start for
            // the same externally supplied pid fails deterministically even
            // when both callers missed the durable row before either was
            // admitted.
            if !self.claimed.write().insert(proc.id().to_string()) {
                // The claim belongs to the admission that installed it: that
                // start releases it itself, either through `Self::abandon`
                // (a start that never became durable) or through `Self::remove`
                // once the row's lifetime ends. Never here.
                return Err(ActError::Action(format!(
                    "proc_id({}) is duplicated in running process list",
                    proc.id()
                )));
            }
            if procs.contains_key(proc.id()) {
                // The pid is resident but its claim was not ours to install: a
                // process loaded from the store is cached without claiming
                // (see `Self::proc`). The claim inserted above stays — it is
                // the resident instance's, and the `remove()` that ends its
                // row lifetime is what releases it.
                return Err(ActError::Action(format!(
                    "proc_id({}) is duplicated in running process list",
                    proc.id()
                )));
            }
            // In-flight reservations are deliberately ignored: they are a
            // hint for restore passes, and were this decision to park on them,
            // a pass that ends up committing fewer rows would strand this
            // process despite a free slot.
            if procs.len() >= self.cap {
                true
            } else {
                procs.insert(proc.id().to_string(), proc.clone());
                false
            }
        };
        if !parked {
            return Ok(true);
        }

        debug!(pid = %proc.id(), "process parked, resident set full");
        let result = self.store.upsert_proc(proc).await;
        if result.is_err() {
            // Like a failed `Process::start`, the parked row never landed, so
            // the directory the start created has no row left to sweep it: it
            // goes with the claim, and before the claim is released, or a
            // start that takes the freed pid could lose the directory it just
            // created.
            remove_workdir(proc.id(), proc.workdir());
            self.claimed.write().remove(proc.id());
        }
        result.map(|_| false)
    }

    /// Commit every free resident slot to the caller for the duration of a
    /// store load. Returns `None` when the set is full — including slots
    /// already committed to another in-flight pass — so a pass whose store I/O
    /// would only find rows another pass is already loading skips the round
    /// trip entirely.
    fn reserve_slots(&self) -> Option<SlotReservation> {
        // `admission` before `procs`, the order every capacity path uses.
        let mut admission = self.admission.lock();
        let resident = self.procs.read().len();
        let free = self.cap.saturating_sub(resident + admission.reserved);
        if free == 0 {
            return None;
        }
        admission.reserved += free;
        Some(SlotReservation {
            cache: self.clone(),
            remaining: free,
        })
    }

    #[instrument(skip(self, rt))]
    pub async fn start_parked(&self, rt: &Arc<Runtime>) -> Result<()> {
        debug!("restore");
        // Queued in-flight rows (boot-resume overflow) outrank parked ones,
        // and the queue only ever holds rows that did not fit the cap — so
        // while it is non-empty there is no free slot for a parked refill, and
        // a concurrent pass must not take one from the queue.
        if !self.pending_resume.read().is_empty() {
            return Ok(());
        }
        // Reserve the free slots before touching the store: concurrent restore
        // passes triggered by other completions then see the set as full and
        // skip, instead of loading — and racing to start — the same parked
        // rows. No lock is held across the store I/O or `Process::start`.
        let Some(mut reservation) = self.reserve_slots() else {
            return Ok(());
        };
        // Nothing is parked, so this pass has nothing to load: skip the
        // resident-set snapshot and the ordered query below. That is the
        // steady state — a resident set that never filled parks nothing, and
        // one whose parked rows all started has none left — and the pass runs
        // on every terminal event, where the snapshot (a `String` per resident
        // process) and the ordered query (the whole `timestamp` index of the
        // collection, materialized only to be intersected with the filter's
        // candidates) are pure waste. The probe is one indexed `state` lookup.
        if !self.store.has_parked().await? {
            return Ok(());
        }
        // Refill free slots from parked rows (`None` state), oldest first —
        // overflow from a full resident set, or never-started seeds. Crashed
        // in-flight rows (`Ready`/`Running`/`Pending`) are the job of
        // [`Self::resume`], the boot pass that also re-dispatches their
        // tasks; this restore only ever runs against a live engine, where
        // non-`None` rows belong to resident processes (reloading one would
        // create a second instance racing it).
        let cached: HashSet<String> = self.procs().iter().map(|p| p.id().to_string()).collect();
        let parked = self
            .store
            .load_parked(reservation.remaining(), rt, &cached)
            .await?;
        for proc in parked {
            // Only the pass that actually makes a process resident starts it:
            // `commit` refuses a row another path already cached, so a
            // duplicate load can never run the workflow twice.
            if reservation.commit(&proc)
                && let Err(err) = proc.start().await
            {
                // A close that landed under this pass refused the dispatch:
                // the row is persisted in flight with its root undispatched —
                // exactly what a crash mid-start leaves — and the next engine
                // start resumes it. Anything else is a real failure of the
                // pass.
                if !err.is_shutdown() {
                    return Err(err);
                }
            }
        }
        Ok(())
    }

    /// Boot-time resume: load processes that were in flight when the engine
    /// crashed (durable `Ready`/`Running`/`Pending` rows) into the resident
    /// set — oldest first, up to `cap`, and ahead of any parked `None` rows —
    /// and return them so [`Runtime::resume`] can re-dispatch their tasks.
    /// Each returned process becomes THE single instance for its pid; a
    /// reloaded in-flight process is re-driven to its next durable
    /// checkpoint by the caller (re-executing a task whose `run` crashed
    /// mid-way is at-least-once, see `Runtime::resume`).
    ///
    /// Rows beyond the cap are NOT stranded: their pids are queued in
    /// [`Self::pending_resume`] and [`Self::resume_from_queue`] loads them
    /// when a terminal event frees a slot — [`Self::start_parked`] only refills
    /// parked (`None`) rows, so without the queue an overflowed in-flight
    /// process would wait forever.
    #[instrument(skip(self, rt))]
    pub(crate) async fn resume(&self, rt: &Arc<Runtime>) -> Result<Vec<Arc<Process>>> {
        debug!("resume");
        // Boot pass, but a terminal event can fire while it runs and drive a
        // concurrent `restore`: reserve slots exactly like `start_parked` so
        // the two never decode — and re-drive — the same durable row.
        let Some(mut reservation) = self.reserve_slots() else {
            // Resident set already at cap (a concurrent pass filled it) —
            // anything else in flight waits in the overflow queue. Whether
            // anything *else* is in flight is the queue's own question: a
            // resident row can be a finished process (one this boot loaded
            // because it still owned an outbox record) that no longer counts
            // as in-flight, and comparing counts would then read "everything
            // is loaded" while an over-cap row waited in the store forever.
            self.enqueue_resume_overflow().await?;
            return Ok(Vec::new());
        };
        let cached: HashSet<String> = self.procs().iter().map(|p| p.id().to_string()).collect();
        let rows = self
            .store
            .load_resumable(reservation.remaining(), rt, &cached)
            .await?;
        let mut resident = Vec::with_capacity(rows.len());
        for proc in rows {
            // Checked and inserted under the write lock, so a row another path
            // cached while we were reading is never loaded a second time.
            if reservation.commit(&proc) {
                resident.push(proc);
            }
        }
        // queue every remaining non-resident in-flight row (rows at or past
        // the cap window, plus any that were skipped above because the
        // resident set was already full) so the terminal-event refill can load
        // them into later-free slots
        self.enqueue_resume_overflow().await?;
        if !resident.is_empty() {
            debug!(loaded = resident.len(), "in-flight processes loaded");
        }
        Ok(resident)
    }

    /// Queue the pids of every durable in-flight row that is not resident —
    /// the boot-resume overflow. `resume_from_queue` drains the queue into
    /// free slots; popping validates the row again (state, residency), so a
    /// pid resumed or removed meanwhile is skipped safely.
    async fn enqueue_resume_overflow(&self) -> Result<()> {
        let resident: HashSet<String> = self.procs().iter().map(|p| p.id().to_string()).collect();
        let query = Query::new()
            .filter(
                Filter::or()
                    .expr(Expr::eq("state", TaskState::Ready.to_string()))
                    .expr(Expr::eq("state", TaskState::Running.to_string()))
                    .expr(Expr::eq("state", TaskState::Pending.to_string())),
            )
            .order("timestamp", Sort::Asc);
        // Exhaustive: a row past a page limit would never be queued, and this
        // queue is the only path that brings a non-resident in-flight process
        // back — the row would sit in the store forever.
        let rows = self.store.procs().query_all(&query).await?;
        let mut queued = 0usize;
        {
            let mut q = self.pending_resume.write();
            for row in rows {
                // `push_back` dedups: a pid an earlier scan already queued is
                // left in place rather than pushed again.
                if !resident.contains(&row.id) && q.push_back(row.id.clone()) {
                    queued += 1;
                }
            }
        }
        if queued > 0 {
            warn!(
                queued,
                "in-flight processes exceed the resident cap — queued to resume as slots free"
            );
        }
        Ok(())
    }

    /// Drain the boot-resume overflow queue into free resident slots, oldest
    /// first. Called by the terminal-event restore ([`Runtime::restore`]) — the
    /// durable rows of queued processes are untouched by [`Self::start_parked`]
    /// (non-`None`), so this is the only path that brings them back. Returns
    /// the newly resident processes for the caller to re-dispatch.
    #[instrument(skip(self, rt))]
    pub(crate) async fn resume_from_queue(&self, rt: &Arc<Runtime>) -> Result<Vec<Arc<Process>>> {
        let mut resident = Vec::new();
        loop {
            let pid = match self.pending_resume.write().pop_front() {
                Some(pid) => pid,
                None => break,
            };
            if self.procs.read().contains_key(&pid) {
                continue;
            }
            // No free slot: put the oldest pid back and stop — the next
            // terminal event retries. Checked against the resident set, not
            // against another pass's in-flight reservations, so a queued
            // in-flight row is never held up by a parked refill.
            if self.procs.read().len() >= self.cap {
                self.pending_resume.write().push_front(pid);
                break;
            }
            let state = match self.store.procs().find_opt(&pid).await? {
                Some(row) => TaskState::from(row.state.as_str()),
                None => continue, // removed while queued
            };
            if !matches!(
                state,
                TaskState::Ready | TaskState::Running | TaskState::Pending
            ) {
                continue; // finished or parked while queued
            }
            if let Some(proc) = self.store.load_proc(&pid, rt).await? {
                // Re-check under the write lock: an admission or another path
                // may have filled the set while the row was being read.
                let mut procs = self.procs.write();
                if procs.contains_key(&pid) {
                    continue;
                }
                if procs.len() >= self.cap {
                    drop(procs);
                    self.pending_resume.write().push_front(pid);
                    break;
                }
                procs.insert(proc.id().to_string(), proc.clone());
                drop(procs);
                debug!(pid = %pid, "queued in-flight process resumed");
                resident.push(proc);
            }
        }
        Ok(resident)
    }

    #[instrument(skip(self, task), fields(pid = %task.pid, tid = %task.id))]
    pub async fn upsert(&self, task: &Arc<Task>) -> Result<()> {
        self.push_task_pri(task, true).await
    }

    fn get_proc(&self, pid: &str) -> Option<Arc<Process>> {
        self.procs.read().get(pid).cloned()
    }

    pub(super) async fn push_proc_pri(&self, proc: &Arc<Process>, save: bool) -> Result<()> {
        debug!("push process pid={}", proc.id());
        if save {
            self.store.upsert_proc(proc).await?;
        }
        self.procs
            .write()
            .insert(proc.id().to_string(), proc.clone());

        Ok(())
    }

    /// Persist a freshly started process and its root task as ONE atomic
    /// store batch, then cache the process and register the root task in
    /// memory — the very first durable write of a process, so a crash can
    /// never leave a durable proc row without its root task row (which would
    /// resume as a task-less, un-runnable process). The rows are durable
    /// before the root task is dispatched to the queue.
    #[instrument(skip(self, proc, root), fields(pid = %proc.id()))]
    pub(crate) async fn start_proc(
        &self,
        proc: &Arc<Process>,
        root: Option<&Arc<Task>>,
    ) -> Result<()> {
        debug!("start process pid={}", proc.id());
        self.store.upsert_proc_with_task(proc, root).await?;
        self.procs
            .write()
            .insert(proc.id().to_string(), proc.clone());
        if let Some(task) = root {
            self.push_task_mem(task)?;
        }
        Ok(())
    }

    pub(super) async fn push_task_pri(&self, task: &Arc<Task>, save: bool) -> Result<()> {
        if save {
            self.persist_task(task).await?;
        }
        self.push_task_mem(task)?;

        Ok(())
    }

    /// Persist a task's state update off the caller's hot path. This is the
    /// bookkeeping of work already admitted, so it waits for room when the
    /// write path is behind instead of being refused.
    #[instrument(skip(self, task), fields(pid = %task.pid, tid = %task.id))]
    pub(crate) async fn upsert_async(&self, task: &Arc<Task>) -> Result<()> {
        self.push_task_mem(task)?;
        self.writer.send(WriteOp::Task(task.clone())).await?;
        Ok(())
    }

    /// Non-blocking persistence for synchronous schedulers. Used only before a
    /// durable `Exec` outbox handoff; when the write path is saturated the
    /// task is refused with [`ActError::QueueFull`] (the caller turns it into
    /// that overflow, or rejects the work) instead of buffering an unbounded
    /// process graph.
    pub(crate) fn try_upsert_async(&self, task: &Arc<Task>) -> Result<()> {
        // Refuse before registering the task in memory: a write the saturated
        // path will not take must not leave a task behind that no durable row
        // backs.
        if self.writer.saturated() {
            return Err(ActError::QueueFull);
        }
        self.push_task_mem(task)?;
        self.writer.try_send(WriteOp::Task(task.clone()))
    }

    /// Try to persist a disk overflow marker for synchronous task admission.
    /// Same refusal contract as [`Self::try_upsert_async`].
    pub(crate) fn try_enqueue_exec(&self, task: &Arc<Task>) -> Result<()> {
        self.writer.try_send(WriteOp::EnqueueExec {
            pid: task.pid.clone(),
            tid: task.id.clone(),
        })?;
        Ok(())
    }

    /// Durable outbox enqueue for a `next` operation, with async writer
    /// backpressure. FIFO order keeps it after the caller's task state write.
    pub(crate) async fn enqueue_next(&self, task: &Arc<Task>) -> Result<()> {
        self.writer
            .send(WriteOp::EnqueueNext {
                pid: task.pid.clone(),
                tid: task.id.clone(),
                target_tid: task.parent_id(),
                source_version: task.timestamp,
            })
            .await
    }

    pub(crate) async fn mark_op_dispatched(&self, op: &crate::data::Op) -> Result<()> {
        self.writer
            .send(WriteOp::MarkOpDispatched {
                pid: op.pid.clone(),
                tid: op.tid.clone(),
                r#type: op.r#type.clone(),
            })
            .await
    }

    pub(crate) async fn mark_op_phase(
        &self,
        pid: &str,
        tid: &str,
        r#type: crate::data::OpType,
        phase: crate::data::OpPhase,
    ) -> Result<()> {
        self.writer
            .send(WriteOp::MarkOpPhase {
                pid: pid.to_string(),
                tid: tid.to_string(),
                r#type: r#type.as_ref().to_string(),
                phase,
            })
            .await
    }

    /// Mark a normal `next` outbox record as overflowed after a bounded queue
    /// rejection. The periodic recovery consumer only replays `Overflow`.
    pub(crate) async fn mark_next_overflow(&self, task: &Arc<Task>) -> Result<()> {
        self.writer
            .send(WriteOp::MarkOpOverflow {
                pid: task.pid.clone(),
                tid: task.id.clone(),
                r#type: crate::data::OpType::Next.as_ref().to_string(),
            })
            .await
    }

    /// Store one message's canonical row and one ack-channel delivery row,
    /// written on the process's shard so the create cannot land after the
    /// close (or the removal) that decides the row's fate — see
    /// [`WriteOp::StoreDelivery`]. `Ok(false)` means nothing was stored: the
    /// process is gone, so the message is delivered without a delivery row
    /// instead of re-creating rows behind its removal.
    pub(crate) async fn store_delivery(
        &self,
        message: &crate::data::Message,
        delivery: &crate::data::Delivery,
    ) -> Result<bool> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.writer
            .send(WriteOp::StoreDelivery {
                message: Box::new(message.clone()),
                delivery: Box::new(delivery.clone()),
                reply: tx,
            })
            .await?;
        rx.await
            .map_err(|_| ActError::Runtime("store writer dropped the delivery".to_string()))?
    }

    /// Close the deliveries of a finished task (deferred to the writer
    /// thread). The store keeps an `Error` row for manual handling.
    pub(crate) async fn close_deliveries(&self, pid: &str, tid: &str) -> Result<()> {
        self.writer
            .send(WriteOp::CloseDeliveries {
                pid: pid.to_string(),
                tid: tid.to_string(),
            })
            .await
    }

    /// Durable outbox enqueue for a client action: the `Pending` record (with
    /// the event + options payload) is queued **before** the action is applied
    /// in memory, so a crash before the state write lands is replayed on
    /// recovery. Deduplicated per `(pid, tid)` against any other in-flight
    /// record.
    pub(crate) async fn enqueue_action(&self, action: &Action) -> Result<()> {
        self.writer
            .send(WriteOp::EnqueueAction {
                pid: action.pid.clone(),
                tid: action.tid.clone(),
                event: action.event.as_ref().to_string(),
                options: action.options.to_string(),
            })
            .await
    }

    /// Durable outbox close for a client action: the state write (and the
    /// message status) were already queued by the caller, so FIFO order makes
    /// `Done` durable only after both.
    pub(crate) async fn complete_action(&self, task: &Arc<Task>) -> Result<()> {
        // a close that reaches the writer after the process was removed would
        // re-create a row nothing deletes: skip it (the removal swept the
        // process's rows, and a closed record is never replayed)
        if self.removed.read().contains(&task.pid) {
            return Ok(());
        }
        // The task/message writes were queued before this call. The durable
        // EffectDurable note is therefore ordered before the outbox close.
        self.writer
            .send(WriteOp::MarkOpPhase {
                pid: task.pid.clone(),
                tid: task.id.clone(),
                r#type: crate::data::OpType::Action.as_ref().to_string(),
                phase: crate::data::OpPhase::EffectDurable,
            })
            .await?;
        self.writer
            .send(WriteOp::OpDone {
                pid: task.pid.clone(),
                tid: task.id.clone(),
                r#type: crate::data::OpType::Action.as_ref().to_string(),
            })
            .await
    }

    /// Close a one-hop propagation after its local effect is durable.
    pub(crate) async fn complete_propagation(
        &self,
        task: &Arc<Task>,
        r#type: crate::data::OpType,
    ) -> Result<()> {
        self.writer
            .send(WriteOp::MarkOpPhase {
                pid: task.pid.clone(),
                tid: task.id.clone(),
                r#type: r#type.as_ref().to_string(),
                phase: crate::data::OpPhase::EffectDurable,
            })
            .await?;
        self.writer
            .send(WriteOp::OpDone {
                pid: task.pid.clone(),
                tid: task.id.clone(),
                r#type: r#type.as_ref().to_string(),
            })
            .await?;
        Ok(())
    }

    /// Durable outbox close: queue the task persist (capturing the applied
    /// propagation phase), then queue the record close after it — FIFO
    /// order makes `Done` durable only after the phase, without blocking the
    /// event loop. If the process crashes between the two, the record is still
    /// `Pending` and recovery re-dispatches it; the durable phase turns the
    /// re-run into a no-op. Safe to call repeatedly: already-closed records
    /// are left untouched.
    pub(crate) async fn complete_next(&self, task: &Arc<Task>) -> Result<()> {
        if self.removed.read().contains(&task.pid) {
            return Ok(());
        }
        self.upsert_async(task).await?;
        self.writer
            .send(WriteOp::MarkOpPhase {
                pid: task.pid.clone(),
                tid: task.id.clone(),
                r#type: crate::data::OpType::Next.as_ref().to_string(),
                phase: crate::data::OpPhase::EffectDurable,
            })
            .await?;
        self.writer
            .send(WriteOp::OpDone {
                pid: task.pid.clone(),
                tid: task.id.clone(),
                r#type: crate::data::OpType::Next.as_ref().to_string(),
            })
            .await?;
        Ok(())
    }

    /// Global writer barrier — test visibility only. Runtime callers never
    /// wait for the whole writer: a cache-miss load barriers the pid's own
    /// shard (`writer.flush_pid`), the sweeper batches one barrier per sweep,
    /// and shutdown drains through `close`.
    #[cfg(test)]
    pub(crate) async fn flush(&self) -> Result<()> {
        self.writer.flush().await
    }

    /// Persist one task: its lifecycle row plus its own vars row when its
    /// scope diverged (scope vars are decoupled from task state writes, so a
    /// pure state transition persists a single small row), then mark the proc
    /// row terminal when the process finished.
    async fn persist_task(&self, task: &Arc<Task>) -> Result<()> {
        self.store.persist_task_rows(task).await?;
        if let Some(p) = task.proc() {
            if p.state().is_completed() {
                self.store
                    .mark_proc_complete(&task.pid, p.end_time(), p.state())
                    .await?;
            }
        } else if task.id == consts::TASK_ROOT_TID && task.state().is_completed() {
            // The process instance was dropped while this write sat queued
            // (the terminal event evicted it under writer backlog): the weak
            // handle no longer upgrades. The root task carries the same truth
            // the proc row needs — its own terminal state is what the proc
            // state mirrors — so the row still converges instead of staying
            // `running` behind a finished workflow forever.
            self.store
                .mark_proc_complete(&task.pid, task.end_time(), task.state())
                .await?;
        }

        Ok(())
    }

    fn push_task_mem(&self, task: &Arc<Task>) -> Result<()> {
        let Some(p) = task.proc() else {
            return Ok(());
        };
        if let Some(proc) = self.procs.read().get(&task.pid).cloned() {
            proc.set_pure_state(p.state());
            proc.set_end_time(p.end_time());
            proc.push_task(task.clone())?;
        }

        Ok(())
    }
}

/// Delete the directory a process's filesystem access was confined to —
/// `<acl workdir root>/<pid>`, as carried in the process's env — when the
/// process named `pid` has one.
///
/// The shape is checked: only a directory whose last component is the pid
/// itself is removed, the only shape a start ever creates, so a row that names
/// something else cannot delete it. Removal is best-effort — a directory holds
/// no engine state, so a failure is reported and never blocks the removal it
/// belongs to (pinning a dead process's rows, and its pid, on a filesystem
/// error would be the worse outcome).
fn remove_workdir(pid: &str, dir: Option<PathBuf>) {
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
