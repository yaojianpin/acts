use super::writer::{StoreWriter, WriteOp};
use crate::{
    ActError, Action, Config, Result,
    data::DeliveryStatus,
    query::{Expr, Filter, Query},
    scheduler::{Process, Runtime, Task, TaskState},
    store::{KvStore, MemoryStore, Store, query::Sort},
};
use parking_lot::RwLock;
use std::{
    collections::{HashMap, HashSet, VecDeque},
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
    /// Boot-resume overflow queue: pids of in-flight (`Ready`/`Running`/
    /// `Pending`) rows that did not fit the resident cap at boot, oldest
    /// first. [`Self::resume_from_queue`] drains them into free slots (the
    /// terminal-event restore) — without it those rows would wait forever,
    /// since [`Self::start_parked`] only refills parked `None` rows.
    pending_resume: Arc<RwLock<VecDeque<String>>>,
    store: Arc<Store>,
    writer: StoreWriter,
    /// Serializes every admission decision and whole restore passes. Terminal
    /// proc events of different processes run concurrently, and admission
    /// (`admit`) and restore both mutate the resident set across store I/O —
    /// without the lock two passes could load — and auto-start — the same
    /// persisted process twice (each `start` is guarded per `Process`
    /// instance, not per pid, so the duplicate would run the workflow twice),
    /// and concurrent starts could overshoot `cap`.
    lock: Arc<tokio::sync::Mutex<()>>,
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
            pending_resume: Arc::new(RwLock::new(VecDeque::new())),
            store: store.clone(),
            writer: StoreWriter::spawn(store),
            lock: Arc::new(tokio::sync::Mutex::new(())),
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

    /// Snapshot of the boot-resume overflow queue (test visibility).
    #[cfg(test)]
    pub(crate) fn pending_resume_ids(&self) -> Vec<String> {
        self.pending_resume.read().iter().cloned().collect()
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

    #[instrument(skip(self, rt), fields(pid = %pid))]
    pub async fn proc(&self, pid: &str, rt: &Arc<Runtime>) -> Result<Option<Arc<Process>>> {
        debug!("process: pid={pid}");
        match self.get_proc(pid) {
            Some(proc) => Ok(Some(proc.clone())),
            None => {
                self.flush().await?;
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
        debug!("remove pid={pid}");
        self.procs.write().remove(pid);
        self.claimed.write().remove(pid);
        // Removal is serialized through the writer (FIFO) so it can never
        // race writes still queued for the process — its completion markers
        // are applied first, then the rows are dropped. `flush` keeps the
        // callers' contract: when this returns, the removal is durable and
        // no pending write can resurrect the rows afterwards.
        self.writer
            .send(WriteOp::RemoveProc {
                pid: pid.to_string(),
            })
            .await?;
        self.writer.flush().await?;
        Ok(true)
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
            self.remove(pid).await?;
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
        // Serialized with restore passes under the same lock: the admission
        // decision mutates the resident set across store I/O on the park
        // path, and a concurrent restore pass must never observe a half-made
        // decision (e.g. start a row whose parking write is still in flight).
        let _guard = self.lock.lock().await;
        // Claim the pid before any store I/O. Both resident and parked
        // admissions leave this marker installed, so a second start for the
        // same externally supplied pid fails deterministically even when both
        // callers missed the durable row before either was admitted.
        if !self.claimed.write().insert(proc.id().to_string()) {
            return Err(ActError::Action(format!(
                "proc_id({}) is duplicated in running process list",
                proc.id()
            )));
        }

        {
            let procs = self.procs.read();
            if procs.contains_key(proc.id()) {
                return Err(ActError::Action(format!(
                    "proc_id({}) is duplicated in running process list",
                    proc.id()
                )));
            }
        }

        if self.count() >= self.cap {
            debug!(pid = %proc.id(), "process parked, resident set full");
            let result = self.store.upsert_proc(proc).await;
            if result.is_err() {
                self.claimed.write().remove(proc.id());
            }
            return result.map(|_| false);
        }

        self.procs
            .write()
            .insert(proc.id().to_string(), proc.clone());
        Ok(true)
    }

    #[instrument(skip(self, rt))]
    pub async fn start_parked(&self, rt: &Arc<Runtime>) -> Result<()> {
        // Terminal proc events of different processes trigger restores
        // concurrently (each completion evicts its process and calls back in
        // here); the pass is serialized with admission (`admit`) under the
        // same lock so two passes can never load — and auto-start — the same
        // persisted process twice (each `start` is guarded per `Process`
        // instance, not per pid, so the duplicate would run the workflow
        // twice), and no start can slip past a half-done pass.
        let _guard = self.lock.lock().await;
        debug!("restore");
        let cap = self.cap();
        let count = self.count();
        if count >= cap {
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
        let free = cap - count;
        for proc in self.store.load_parked(free, rt, &cached).await? {
            if !self.procs.read().contains_key(proc.id()) {
                self.push_proc_pri(&proc, false).await?;
                proc.start().await?;
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
        let _guard = self.lock.lock().await;
        debug!("resume");
        let cap = self.cap();
        let count = self.count();
        let mut resident = Vec::new();
        let cached: HashSet<String> = self.procs().iter().map(|p| p.id().to_string()).collect();
        if count < cap {
            let free = cap - count;
            let procs = self.store.load_resumable(free, rt, &cached).await?;
            resident.reserve(procs.len());
            for proc in procs {
                if !self.procs.read().contains_key(proc.id()) {
                    self.procs
                        .write()
                        .insert(proc.id().to_string(), proc.clone());
                    resident.push(proc);
                }
            }
            // queue every remaining non-resident in-flight row (rows at or
            // past the cap window, plus any that were skipped above because
            // the resident set was already full) so the terminal-event refill
            // can load them into later-free slots
            if cached.len() + resident.len() < self.store.count_resumable().await? {
                self.enqueue_resume_overflow().await?;
            }
        } else if self.store.count_resumable().await? > cached.len() {
            // resident set already at cap (the outbox replay cached its own
            // processes before this ran) — anything else in flight waits
            self.enqueue_resume_overflow().await?;
        }
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
        let page = self.store.procs().query(&query).await?;
        let mut queued = 0usize;
        {
            let mut q = self.pending_resume.write();
            for row in page.rows {
                if !resident.contains(&row.id) {
                    q.push_back(row.id.clone());
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
        let _guard = self.lock.lock().await;
        let mut resident = Vec::new();
        loop {
            let pid = match self.pending_resume.write().pop_front() {
                Some(pid) => pid,
                None => break,
            };
            if self.count() >= self.cap {
                self.pending_resume.write().push_front(pid);
                break;
            }
            if self.procs.read().contains_key(&pid) {
                continue;
            }
            let state = match self.store.procs().find(&pid).await {
                Ok(row) => TaskState::from(row.state.as_str()),
                Err(_) => continue, // removed while queued
            };
            if !matches!(
                state,
                TaskState::Ready | TaskState::Running | TaskState::Pending
            ) {
                continue; // finished or parked while queued
            }
            if let Some(proc) = self.store.load_proc(&pid, rt).await? {
                self.procs
                    .write()
                    .insert(proc.id().to_string(), proc.clone());
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

    #[instrument(skip(self, task), fields(pid = %task.pid, tid = %task.id))]
    pub(crate) async fn upsert_async(&self, task: &Arc<Task>) -> Result<()> {
        self.push_task_mem(task)?;
        self.writer.send(WriteOp::Task(task.clone())).await?;
        Ok(())
    }

    /// Non-blocking persistence for synchronous schedulers. Used only before a
    /// durable `Exec` outbox handoff; if the writer is full the caller rejects
    /// the work explicitly instead of buffering an unbounded process graph.
    pub(crate) fn try_upsert_async(&self, task: &Arc<Task>) -> Result<()> {
        self.push_task_mem(task)?;
        self.writer.try_send(WriteOp::Task(task.clone()))
    }

    /// Try to persist a disk overflow marker for synchronous task admission.
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

    pub(crate) async fn upsert_message_status(
        &self,
        pid: &str,
        tid: &str,
        status: DeliveryStatus,
    ) -> Result<()> {
        self.writer
            .send(WriteOp::DeliveryStatus {
                pid: pid.to_string(),
                tid: tid.to_string(),
                status,
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
        self.writer
            .send(WriteOp::OpDone {
                pid: task.pid.clone(),
                tid: task.id.clone(),
                r#type: crate::data::OpType::Action.as_ref().to_string(),
            })
            .await
    }

    /// Durable outbox close: queue the task persist (capturing the
    /// `NEXT_COMPLETE` marker), then queue the record close after it — FIFO
    /// order makes `Done` durable only after the marker, without blocking the
    /// event loop. If the process crashes between the two, the record is still
    /// `Pending` and recovery re-dispatches it; the durable marker turns the
    /// re-run into a no-op. Safe to call repeatedly: already-closed records
    /// are left untouched.
    pub(crate) async fn complete_next(&self, task: &Arc<Task>) -> Result<()> {
        self.upsert_async(task).await?;
        self.writer
            .send(WriteOp::OpDone {
                pid: task.pid.clone(),
                tid: task.id.clone(),
                r#type: crate::data::OpType::Next.as_ref().to_string(),
            })
            .await?;
        Ok(())
    }

    pub(crate) async fn flush(&self) -> Result<()> {
        self.writer.flush().await
    }

    /// Persist one task: its lifecycle row plus the vars rows of every dirty
    /// scope on its parent chain (scope vars are decoupled from task state
    /// writes, so a pure state transition persists a single small row), then
    /// mark the proc row terminal when the process finished.
    async fn persist_task(&self, task: &Arc<Task>) -> Result<()> {
        self.store.persist_task_rows(task).await?;
        if let Some(p) = task.proc()
            && p.state().is_completed()
        {
            self.store
                .mark_proc_complete(&task.pid, p.end_time(), p.state())
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
