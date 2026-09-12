use super::validation::SchemaCache;
use super::{ActTask, Context, Process, Sign, Task, TaskState};
use crate::snapshot::{SnapshotOptions, SnapshotStore};
use crate::{
    ActError, Action, Config, Error, Package, Result, ShareLock, Vars, Workflow,
    cache::Cache,
    data,
    env::Enviroment,
    event::{Emitter, EventAction, ProcessGate},
    scheduler::queue::{Queue, QueueData},
    store::{KvStore, Store},
    utils::{self, consts},
};
use parking_lot::RwLock;
use std::{
    any::Any,
    collections::HashMap,
    future::Future,
    panic::{AssertUnwindSafe, catch_unwind},
    pin::Pin,
    sync::Arc,
    task::{Context as TaskContext, Poll},
    time::Duration,
};
use tokio::{runtime::Handle, sync::mpsc, time};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, instrument};

#[derive(Clone)]
pub struct Runtime {
    config: Arc<Config>,
    queue: Arc<Queue>,
    env: Arc<Enviroment>,
    cache: Arc<Cache>,
    emitter: Arc<Emitter>,
    package: Arc<Package>,
    shutdown: CancellationToken,
    schema_cache: Arc<SchemaCache>,
    pub(crate) snapshots: Arc<SnapshotRegistry>,
}

/// Registry of snapshot-backed sealed-data targets (see [`crate::snapshot`]).
pub(crate) struct SnapshotRegistry {
    stores: ShareLock<HashMap<String, Arc<SnapshotStore>>>,
}

/// A unit accepted by the scheduler lane pool. The original queue item's
/// process lease is carried through dispatch so a finished/evicted process
/// cannot invalidate work that was already admitted.
#[derive(Debug)]
enum SchedulerJob {
    Exec { task: Arc<Task>, proc: Arc<Process> },
    Next { task: Arc<Task>, proc: Arc<Process> },
}

/// Fixed set of serial workers. Jobs are assigned by pid hash, so all work for
/// one process is queued on the same lane and preserves FIFO order while
/// independent processes can run on different lanes.
#[derive(Debug)]
struct SchedulerLanes {
    senders: Vec<mpsc::UnboundedSender<SchedulerJob>>,
    gate: ProcessGate,
}

impl SchedulerLanes {
    fn new(count: usize, emitter: Arc<Emitter>, shutdown: CancellationToken) -> Self {
        let count = count.max(1);
        let gate = emitter.process_gate();
        let mut senders = Vec::with_capacity(count);
        for _ in 0..count {
            let (sender, mut receiver) = mpsc::unbounded_channel::<SchedulerJob>();
            let shutdown = shutdown.clone();
            let gate = emitter.process_gate();
            tokio::spawn(async move {
                loop {
                    let job = tokio::select! {
                        _ = shutdown.cancelled() => break,
                        job = receiver.recv() => job,
                    };
                    let Some(job) = job else { break };
                    let pid = match &job {
                        SchedulerJob::Exec { task, .. } | SchedulerJob::Next { task, .. } => {
                            task.pid.clone()
                        }
                    };
                    let gate = gate.lock(&pid).await;
                    Runtime::execute_job(job).await;
                    drop(gate);
                }
            });
            senders.push(sender);
        }
        Self { senders, gate }
    }

    fn dispatch(&self, job: SchedulerJob) -> crate::Result<()> {
        let pid = match &job {
            SchedulerJob::Exec { task, .. } | SchedulerJob::Next { task, .. } => &task.pid,
        };
        // The event gate uses the same stable FNV-1a mapping; this ensures a
        // process's user event handlers cannot interleave with its lane job.
        let lane = self.gate.lane_index(pid);
        self.senders[lane]
            .send(job)
            .map_err(|err| ActError::Runtime(err.to_string()))
    }
}

/// Catches panics raised while polling an operation, without moving that
/// operation to another task (which would change scheduler event ordering).
struct CatchPanic<F> {
    future: Option<F>,
}

impl<F> CatchPanic<F> {
    fn new(future: F) -> Self {
        Self {
            future: Some(future),
        }
    }
}

impl<F: Future> Future for CatchPanic<F> {
    type Output = std::result::Result<F::Output, Box<dyn Any + Send>>;

    fn poll(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        // The future is structurally pinned with the wrapper and is never moved
        // or replaced after polling starts.
        let this = unsafe { self.get_unchecked_mut() };
        let Some(future) = this.future.as_mut() else {
            panic!("CatchPanic was polled after completion");
        };
        let future = unsafe { Pin::new_unchecked(future) };

        match catch_unwind(AssertUnwindSafe(move || future.poll(cx))) {
            Ok(Poll::Ready(value)) => {
                this.future = None;
                Poll::Ready(Ok(value))
            }
            Ok(Poll::Pending) => Poll::Pending,
            Err(payload) => {
                this.future = None;
                Poll::Ready(Err(payload))
            }
        }
    }
}

impl SnapshotRegistry {
    fn new() -> Self {
        Self {
            stores: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.stores.read().len()
    }

    /// Register (or replace) a snapshot target. Replacing a name drops its
    /// cached values.
    pub(crate) fn register(&self, name: &str, options: SnapshotOptions) -> Arc<SnapshotStore> {
        let store = Arc::new(SnapshotStore::new(options));
        self.stores.write().insert(name.to_string(), store.clone());
        store
    }

    pub(crate) fn store(&self, name: &str) -> Option<Arc<SnapshotStore>> {
        self.stores.read().get(name).cloned()
    }

    pub(crate) fn list(&self) -> Vec<(String, Arc<SnapshotStore>)> {
        self.stores
            .read()
            .iter()
            .map(|(name, store)| (name.clone(), store.clone()))
            .collect()
    }
    /// Drop expired entries of every registered store (TTL sweep).
    pub(crate) fn purge_expired(&self) -> usize {
        self.list()
            .into_iter()
            .map(|(_, store)| store.purge_expired())
            .sum()
    }
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime")
            .field("config", &self.config)
            .field("queue", &self.queue)
            .field("env", &self.env)
            .field("cache", &self.cache)
            .field("emitter", &self.emitter)
            .field("package", &self.package)
            .field(
                "schema_cache",
                &format_args!("<{} entries>", self.schema_cache.len()),
            )
            .field(
                "snapshots",
                &format_args!("<{} entries>", self.snapshots.len()),
            )
            .finish()
    }
}

impl Runtime {
    pub(crate) fn snapshot_registry(&self) -> Arc<SnapshotRegistry> {
        self.snapshots.clone()
    }

    pub fn new(config: &Config, store: Option<Arc<dyn KvStore>>) -> crate::Result<Arc<Self>> {
        let runtime = Self::create(config, store)?;
        Ok(runtime)
    }

    #[allow(unused)]
    pub fn cache(&self) -> &Arc<Cache> {
        &self.cache
    }

    #[allow(unused)]
    pub fn queue(&self) -> &Arc<Queue> {
        &self.queue
    }

    #[allow(unused)]
    pub fn env(&self) -> &Arc<Enviroment> {
        &self.env
    }

    pub fn emitter(&self) -> &Arc<Emitter> {
        &self.emitter
    }

    pub fn package(&self) -> &Arc<Package> {
        &self.package
    }

    pub(crate) fn schema_cache(&self) -> &Arc<SchemaCache> {
        &self.schema_cache
    }

    pub(crate) async fn package_definition(
        &self,
        uses: &str,
    ) -> crate::Result<Arc<super::validation::CachedPackage>> {
        let store = self.store();
        self.schema_cache.package(&store, uses).await
    }

    pub fn store(&self) -> Arc<Store> {
        self.cache.store().clone()
    }

    /// Bounded scheduler-queue metrics: capacity, current depth, and high
    /// watermark. The durable outbox backlog is visible through pending ops.
    pub fn scheduler_queue_capacity(&self) -> usize {
        self.queue.capacity()
    }

    pub fn scheduler_queue_depth(&self) -> usize {
        self.queue.depth()
    }

    pub fn scheduler_queue_high_watermark(&self) -> usize {
        self.queue.high_watermark()
    }

    pub fn store_writer_depth(&self) -> usize {
        self.cache.store_writer_depth()
    }

    pub fn store_writer_high_watermark(&self) -> usize {
        self.cache.store_writer_high_watermark()
    }

    #[allow(unused)]
    pub fn config(&self) -> &Arc<Config> {
        &self.config
    }
    pub(crate) fn register_snapshot(
        &self,
        name: &str,
        options: SnapshotOptions,
    ) -> Arc<SnapshotStore> {
        self.snapshots.register(name, options)
    }

    pub(crate) fn snapshot_store(&self, name: &str) -> Option<Arc<SnapshotStore>> {
        self.snapshots.store(name)
    }

    pub async fn close(&self) {
        self.shutdown.cancel();
        self.queue.abort();
        self.cache.close().await;
        self.emitter.close();
    }

    pub(crate) fn shutdown_token(&self) -> CancellationToken {
        self.shutdown.clone()
    }

    /// Start a workflow process.
    ///
    /// An externally supplied pid is unique within this runtime instance.
    /// Deployments that run multiple runtime instances against one store must
    /// enforce external pid uniqueness at their boundary.
    #[instrument(skip(self, model, options), fields(mid = %model.id, name = %model.name))]
    pub async fn start(self: &Arc<Self>, model: &Workflow, options: Vars) -> Result<Arc<Process>> {
        debug!("process starting");

        let mut proc_id = utils::longid();
        if let Some(pid) = &options.get::<String>(consts::PROCESS_ID) {
            // the pid will use as the proc_id
            proc_id = pid.to_string();

            // check external pid is valid
            if proc_id.is_empty() {
                return Err(ActError::Action(
                    "external process id cannot be empty".to_string(),
                ));
            }

            if proc_id.contains(consts::KEY_SEP) {
                return Err(ActError::Action(format!(
                    "external process id cannot contain '{}'",
                    consts::KEY_SEP
                )));
            }
        }
        let proc = self.cache.proc(&proc_id, self).await?;
        if proc.is_some() {
            return Err(ActError::Action(format!(
                "proc_id({proc_id}) is duplicated in running process list"
            )));
        }

        // validate the options
        if !model.inputs.is_empty() {
            model
                .inputs
                .validate(&(options.to_value()))
                .map_err(|err| {
                    ActError::Model(format!(
                        "model({}) inputs validation error: {}",
                        model.id, err
                    ))
                })?;
        }

        let proc = Process::new(&proc_id, self);
        proc.load_with_vars(model, &options)?;

        self.launch(&proc).await?;

        if proc.state().is_none() {
            info!(pid = %proc_id, mid = %model.id, name = %model.name, "process parked — waiting for a free slot");
        } else {
            info!(pid = %proc_id, mid = %model.id, name = %model.name, "process started");
        }
        Ok(proc)
    }

    pub async fn proc(self: &Arc<Self>, pid: &str) -> Result<Option<Arc<Process>>> {
        self.cache.proc(pid, self).await
    }

    #[instrument(skip(self, proc), fields(pid = %proc.id()))]
    pub async fn launch(self: &Arc<Self>, proc: &Arc<Process>) -> Result<()> {
        debug!("process launched");
        let proc = proc.clone();
        // Capacity admission: when the resident set is full the process is
        // *parked* (its durable row stays `None`) and started later by the
        // restore pass that follows a terminal event — a running process is
        // never evicted from memory to make room. A parked `start` returns
        // here; `Process::start` itself is what runs the workflow.
        if !self.cache.admit(&proc).await? {
            return Ok(());
        }
        if let Err(err) = proc.start().await {
            self.cache.evict(proc.id());
            return Err(err);
        }
        Ok(())
    }

    #[allow(unused)]
    pub(crate) fn create_proc(self: &Arc<Self>, pid: &str, model: &Workflow) -> Arc<Process> {
        let proc = Process::new(pid, self);
        proc.load(model);
        proc
    }

    #[instrument(skip(self, task), fields(pid = %task.pid, tid = %task.id))]
    pub fn push(&self, task: &Arc<Task>) -> Result<()> {
        debug!("task pushed");
        let cache = self.cache.clone();
        let task_clone = task.clone();
        cache.try_upsert_async(&task_clone)?;
        match self.queue.send(&task_clone) {
            Ok(()) => Ok(()),
            // The task row is queued first; the `Exec` outbox record becomes
            // the disk queue. Keep it pending for the retry timer.
            Err(ActError::QueueFull) => {
                cache.try_enqueue_exec(&task_clone)?;
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    /// Dispatch a task to the in-memory queue WITHOUT queueing another store
    /// write — used for the root task of a freshly started process, whose
    /// proc row + root task row were already persisted atomically by
    /// `Cache::start_proc`.
    #[instrument(skip(self, task), fields(pid = %task.pid, tid = %task.id))]
    pub(crate) fn dispatch_root(&self, task: &Arc<Task>) -> Result<()> {
        debug!("root task dispatched");
        match self.queue.send(task) {
            Ok(()) => Ok(()),
            // The root row is durable before dispatch; overflow is a descriptor.
            Err(ActError::QueueFull) => {
                self.cache.try_enqueue_exec(task)?;
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    #[instrument(skip(self, action), fields(pid = %action.pid, tid = %action.tid, event = ?action.event))]
    pub async fn do_action(self: &Arc<Self>, action: &Action) -> Result<()> {
        debug!("action received");
        let proc = self.cache.proc(&action.pid, self).await?;
        match proc {
            Some(proc) => proc.do_action(action).await,
            None => Err(ActError::Runtime(format!(
                "cannot find process '{}' when do_action({:?})",
                action.pid, action
            ))),
        }
    }

    /// Durable outbox enqueue for a `next` operation: a `Pending` outbox record
    /// is queued on the store writer (after the task state change, so the task
    /// is durable first) and the operation is dispatched to the bounded
    /// in-memory queue. This scheduler path applies backpressure; a crash
    /// before the record lands is
    /// consistent (nothing to replay); a crash after it lands is recovered by
    /// [`Self::recover_actions`]; a crash after the operation ran is a no-op
    /// thanks to the durably persisted `NEXT_COMPLETE` marker.
    pub(crate) async fn enqueue_next(&self, task: &Arc<Task>) -> Result<()> {
        self.cache.enqueue_next(task).await?;
        match self.queue.send_next(task) {
            Ok(()) => Ok(()),
            // Convert the existing normal `Next` record into the disk queue's
            // overflow state. The periodic recovery consumer owns it until it
            // is successfully handed back to memory.
            Err(ActError::QueueFull) => {
                self.cache.mark_next_overflow(task).await?;
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    /// Durable outbox close for a task whose `next` propagation finished: queue
    /// the task persist (with the `NEXT_COMPLETE` marker) and then the outbox
    /// record close, in order, on the store writer — non-blocking. Called from
    /// `Task::next` once the task reaches a terminal state (also for the
    /// idempotent replay guard), and from the event loop when `next` ends in
    /// error. Non-terminal outcomes (children in flight, interrupt) leave the
    /// record `Pending` so recovery replays it.
    pub(crate) async fn complete_next(&self, task: &Arc<Task>) -> Result<()> {
        self.cache.complete_next(task).await
    }

    /// Durable outbox enqueue for a client action (non-`Next` events): the
    /// `Pending` record with the event + options payload is written **before**
    /// the action is applied, so a crash before the task state write lands is
    /// replayed by [`Self::recover_actions`].
    pub(crate) async fn enqueue_action(&self, action: &Action) -> Result<()> {
        self.cache.enqueue_action(action).await
    }

    /// Durable outbox close for a client action: the state write and message
    /// status were already queued by the caller, so FIFO order makes `Done`
    /// durable only after both.
    pub(crate) async fn complete_action(&self, task: &Arc<Task>) -> Result<()> {
        self.cache.complete_action(task).await
    }

    /// Replay durable outbox records that were not durably completed (the
    /// engine crashed before the queued `next` ran, before its effects were
    /// persisted, or before a client action's state write became durable).
    /// Re-enqueueing is idempotent:
    /// - `next` records of a task whose `next` already completed are skipped
    ///   by the durable `NEXT_COMPLETE` guard and closed;
    /// - `next` records of a task whose `next` never ran are dispatched again,
    ///   and re-scheduling is deduplicated against tasks created before the
    ///   crash;
    /// - action records of a task that is already in a terminal state are
    ///   closed (the action was applied durably) and the task's messages are
    ///   marked completed so the client is not asked to act again — except
    ///   `Cancel`/`Remove`, which never guard on the target's state (a Cancel
    ///   target is usually already `Completed`), so they are always re-applied
    ///   and an already-applied one is rejected by the arm's guards;
    /// - action records of a task that never received the action are
    ///   re-applied, which also closes the record through the action path.
    pub async fn recover_actions(self: &Arc<Self>) -> Result<()> {
        let ops = self.cache.store().load_pending_ops().await?;
        for op in ops {
            let r#type = op.r#type.clone();
            let (pid, tid) = (op.pid.clone(), op.tid.clone());
            let Some(proc) = self.cache.proc(&pid, self).await? else {
                // process is gone (removed while completing) — drop the orphan
                self.cache.store().complete_ops(&pid, &tid, &r#type).await?;
                continue;
            };
            let Some(task) = proc.task(&tid) else {
                self.cache.store().complete_ops(&pid, &tid, &r#type).await?;
                continue;
            };
            if r#type == data::OpType::Action.as_ref() {
                let (Some(event), Some(options)) = (op.event.as_deref(), op.options.as_deref())
                else {
                    // malformed action record — drop it
                    self.cache.store().complete_ops(&pid, &tid, &r#type).await?;
                    continue;
                };
                let Ok(event) = EventAction::parse(event) else {
                    error!(pid = %pid, tid = %tid, event = %event, "cannot parse replayed action");
                    self.cache.store().complete_ops(&pid, &tid, &r#type).await?;
                    continue;
                };
                let Ok(options) = serde_json::from_str::<Vars>(options) else {
                    error!(pid = %pid, tid = %tid, "cannot parse replayed action options");
                    self.cache.store().complete_ops(&pid, &tid, &r#type).await?;
                    continue;
                };
                // `Cancel` and `Remove` never guard on the target task's state
                // (a Cancel target is usually already `Completed` from an
                // earlier `Next`; Remove has no guard at all), so a terminal
                // target does NOT prove the action was applied — always
                // re-apply them. Re-applying an already-applied one is
                // rejected by the arm's guards and closes the record.
                let always_reapply = matches!(event, EventAction::Cancel | EventAction::Remove);
                if !always_reapply && task.state().is_completed() {
                    // already applied durably (the state write landed but the
                    // close was lost) — close and mark the messages completed
                    self.cache.store().complete_ops(&pid, &tid, &r#type).await?;
                    self.cache
                        .store()
                        .set_deliveries_with(&pid, &tid, data::DeliveryStatus::Completed)
                        .await?;
                    continue;
                }
                // the action was never durably applied — re-apply it; the
                // action path (Task::update) closes the record itself
                let action = Action::new(&pid, &tid, event, options);
                if let Err(err) = proc.do_action(&action).await {
                    error!(error = %err, pid = %pid, tid = %tid, "replayed action failed");
                    self.cache.store().complete_ops(&pid, &tid, &r#type).await?;
                }
            } else if r#type == data::OpType::Exec.as_ref() {
                // Overflow task execution replay: do not confuse it with `next`.
                if !task.state().is_completed() {
                    if let Err(ActError::QueueFull) = self.queue.send(&task) {
                        continue;
                    }
                    if let Err(err) = self.cache.mark_op_dispatched(&op).await {
                        error!(error = %err, pid = %pid, tid = %tid, "failed to mark replayed exec dispatched");
                    }
                } else {
                    self.cache.store().complete_ops(&pid, &tid, &r#type).await?;
                }
            } else if task.is_sign(Sign::NEXT_COMPLETE) {
                // propagation already completed durably; just close the record
                // and mark the deliveries completed
                self.cache.store().complete_ops(&pid, &tid, &r#type).await?;
                self.cache
                    .store()
                    .set_deliveries_with(&pid, &tid, data::DeliveryStatus::Completed)
                    .await?;
                continue;
            } else {
                match self.queue.send_next(&task) {
                    Ok(()) => {
                        if let Err(err) = self.cache.mark_op_dispatched(&op).await {
                            error!(error = %err, pid = %pid, tid = %tid, "failed to mark replayed next dispatched");
                        }
                    }
                    // On restart into an already full queue, retain overflow.
                    Err(ActError::QueueFull) => continue,
                    Err(err) => return Err(err),
                }
            }
        }
        Ok(())
    }

    /// Replay durable overflow records produced while the bounded scheduler
    /// queue was full. This is the disk-queue consumer for overload: records
    /// stay pending on disk and are handed back to memory only after they age
    /// past one tick, giving the normal queue time to drain.
    async fn recover_overflow(self: &Arc<Self>, older_than_millis: i64) -> Result<()> {
        let store = self.cache.store();
        for op in store.load_overflow_ops(older_than_millis).await? {
            let r#type = op.r#type.clone();
            let is_exec_overflow = r#type == data::OpType::Exec.as_ref()
                && op.status == data::OpStatus::Pending.as_ref();
            let is_next_overflow = r#type == data::OpType::Next.as_ref()
                && op.status == data::OpStatus::Overflow.as_ref();
            if !is_exec_overflow && !is_next_overflow {
                continue;
            }

            let (pid, tid) = (op.pid.clone(), op.tid.clone());
            let Some(proc) = self.cache.proc(&pid, self).await? else {
                store.complete_ops(&pid, &tid, &r#type).await?;
                continue;
            };
            let Some(task) = proc.task(&tid) else {
                store.complete_ops(&pid, &tid, &r#type).await?;
                continue;
            };

            if task.state().is_completed() {
                store.complete_ops(&pid, &tid, &r#type).await?;
                continue;
            }

            let queued = if is_next_overflow {
                self.queue.send_next(&task)
            } else {
                self.queue.send(&task)
            };
            match queued {
                Ok(()) => {
                    if let Err(err) = self.cache.mark_op_dispatched(&op).await {
                        error!(error = %err, pid = %pid, tid = %tid, "failed to mark overflow op dispatched");
                    }
                }
                // Still full: leave the small durable descriptor pending.
                Err(ActError::QueueFull) => {}
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }

    /// Boot-time resume of processes that were in flight when the engine
    /// crashed: load their durable `Ready`/`Running`/`Pending` rows into the
    /// resident set first (up to `cap`, oldest first), then re-dispatch every
    /// task that was cut off mid-flight through the normal queue so
    /// `exec`/`next` carry it to its next durable checkpoint (at-least-once).
    ///
    /// A task is only re-dispatched when it has NO durable outbox record
    /// pending: a task with one was already past its `run` (the record is
    /// written after `exec` and closed at its terminal state), so
    /// [`Self::recover_actions`] re-drives its propagation instead — re-
    /// running it here would re-enter the parent's scheduling and duplicate
    /// completed siblings (`schedule_once` treats a terminal instance as
    /// "redo me"). Tasks waiting on a client action (`Interrupt`) or on
    /// sibling branches (`Pending`) are not dispatched either.
    pub(crate) async fn resume(self: &Arc<Self>) -> Result<()> {
        let procs = self.cache.resume(self).await?;
        // scan the whole resident set: `procs` only holds the rows freshly loaded
        // here, but the outbox replay above already cached its own processes —
        // their op-less mid-flight leaves need the same re-drive
        let residents = self.cache.procs();
        let redispatched = self.redispatch_resumed(&residents).await?;
        if redispatched > 0 {
            info!(
                resumed = procs.len(),
                redispatched, "in-flight processes resumed after restart"
            );
        }
        // start parked (`None`) processes into the remaining free slots
        self.cache.start_parked(self).await?;
        Ok(())
    }

    /// Terminal-event restore: a process just finished and its terminal event
    /// evicted it. Loads queued in-flight rows (boot-resume overflow that did
    /// not fit the cap) into the freed slots and re-dispatches them, then
    /// refills parked (`None`) rows — non-`None` first, matching boot
    /// priority.
    pub(crate) async fn restore(self: &Arc<Self>) -> Result<()> {
        let loaded = self.cache.resume_from_queue(self).await?;
        if !loaded.is_empty() {
            let redispatched = self.redispatch_resumed(&loaded).await?;
            info!(
                loaded = loaded.len(),
                redispatched, "queued in-flight processes resumed"
            );
        }
        self.cache.start_parked(self).await?;
        Ok(())
    }

    /// Re-dispatch the op-less mid-flight tasks of every resident process:
    /// tasks with a pending outbox record are driven by `recover_actions` —
    /// a task with one was already past its `run` (the record is written
    /// after `exec` and closed at its terminal state), so re-running it here
    /// would re-enter the parent's scheduling and duplicate completed
    /// siblings (`schedule_once` treats a terminal instance as "redo me").
    /// Of the rest, `None`/`Ready` tasks run from their entry, while a
    /// `Running` task cut off mid-run is reset to `Ready` first — `exec`
    /// only re-runs `run` for a `Ready` task — but only when it is a leaf: a
    /// running parent's durable children are resumed on their own and drive
    /// it to completion when they finish. Tasks waiting on a client action
    /// (`Interrupt`) or on sibling branches (`Pending`) are not dispatched.
    async fn redispatch_resumed(&self, procs: &[Arc<Process>]) -> Result<usize> {
        let ops = self.cache.store().load_pending_ops().await?;
        let in_flight: std::collections::HashSet<(String, String)> = ops
            .iter()
            .map(|op| (op.pid.clone(), op.tid.clone()))
            .collect();
        let mut redispatched = 0usize;
        for proc in procs {
            debug!(pid = %proc.id(), state = ?proc.state(), "process resumed");
            for task in proc.tasks() {
                if in_flight.contains(&(proc.id().to_string(), task.id.clone())) {
                    continue;
                }
                let task = task.clone();
                let state = task.state();
                let redispatched_task = match state {
                    TaskState::None | TaskState::Ready => Some(task),
                    TaskState::Running if task.children().is_empty() => {
                        task.set_pure_state(TaskState::Ready);
                        Some(task)
                    }
                    _ => None,
                };
                if let Some(task) = redispatched_task {
                    self.push(&task)?;
                    redispatched += 1;
                }
            }
        }
        Ok(redispatched)
    }

    #[cfg(test)]
    pub async fn do_action2(
        self: &Arc<Self>,
        pid: &str,
        tid: &str,
        action: EventAction,
        options: crate::Vars,
    ) -> Result<()> {
        self.do_action(&Action::new(pid, tid, action, options))
            .await
    }

    /// Ack one delivery row (by its delivery id).
    pub async fn ack(&self, id: &str) -> Result<()> {
        self.cache
            .store()
            .set_delivery(id, data::DeliveryStatus::Acked)
            .await
    }

    pub fn event_loop(self: &Arc<Self>) {
        let queue = self.queue.clone();
        let emitter = self.emitter.clone();
        let shutdown = self.shutdown.clone();
        // The dispatcher only dequeues/admits work. Fixed serial lanes provide
        // the explicit in-flight cap while preserving per-pid ordering.
        let lanes =
            SchedulerLanes::new(self.config().scheduler_workers(), emitter, shutdown.clone());
        tokio::spawn(async move {
            // If this future is dropped or unwinds unexpectedly, make the
            // failure visible to producers (`queue.send`) instead of letting an
            // unbounded queue accumulate work for a dead scheduler.
            let _consumer = queue.consumer_lease();
            loop {
                let next = tokio::select! {
                    _ = shutdown.cancelled() => break,
                    next = queue.next() => next,
                };
                match next {
                    Ok(data) => match data {
                        QueueData::Task { task, proc } => {
                            if let Err(err) = lanes.dispatch(SchedulerJob::Exec { task, proc }) {
                                error!(error = %err, "scheduler lane dispatch failed");
                            }
                        }
                        QueueData::Next { task, proc } => {
                            if let Err(err) = lanes.dispatch(SchedulerJob::Next { task, proc }) {
                                error!(error = %err, "scheduler lane dispatch failed");
                            }
                        }
                        QueueData::Abort => {
                            break;
                        }
                    },
                    Err(err) => {
                        error!(error = %err, "queue.next failed");
                        break;
                    }
                }
            }
        });
    }

    /// Build an execution context while catching a panic at the poll boundary.
    async fn execute_job(job: SchedulerJob) {
        let (task, proc, operation) = match job {
            SchedulerJob::Exec { task, proc } => (task, proc, "task.exec"),
            SchedulerJob::Next { task, proc } => (task, proc, "task.next"),
        };
        let Some(ctx) = Self::isolate_context(task.clone(), proc).await else {
            return;
        };
        let reporting_task = task.clone();
        let reporting_ctx = ctx.clone();
        let result = CatchPanic::new(async move {
            match operation {
                "task.exec" => Self::run_exec_job(task, ctx).await,
                _ => Self::run_next_job(task, ctx).await,
            }
        })
        .await;
        Self::report_task_panic(operation, reporting_task, reporting_ctx, result).await;
    }

    async fn run_exec_job(task: Arc<Task>, ctx: Context) {
        if let Err(err) = task.exec(&ctx).await {
            error!(error = %err, "task.exec failed");
            task.set_err(&err.clone().into());
            ctx.set_task(&task);
            ctx.emit_error().await.ok();
        }
    }

    async fn run_next_job(task: Arc<Task>, ctx: Context) {
        let result = task.next(&ctx).await;
        if let Err(err) = result {
            error!(error = %err, "task.next failed");
            task.set_err(&err.clone().into());
            ctx.set_task(&task);
            ctx.emit_error().await.ok();
            // the propagation ended in error (terminal):
            // close the outbox record so recovery does not
            // replay the failed `next`
            if let Err(err) = task.runtime().complete_next(&task).await {
                error!(error = %err, "complete_next failed");
            }
        }
        // On success the record is closed inside `next` once the task reaches
        // a terminal state; outcomes with children still in flight or an
        // interrupt leave it `Pending` for recovery to replay.
    }

    async fn isolate_context(task: Arc<Task>, proc: Arc<Process>) -> Option<Context> {
        // Keep the queue item's process lease alive through context setup.
        let _proc = proc;
        match catch_unwind(AssertUnwindSafe(|| task.create_context())) {
            Ok(ctx) => Some(ctx),
            Err(payload) => {
                error!(
                    error = %Self::panic_payload_error(payload),
                    "task context creation panicked"
                );
                None
            }
        }
    }

    /// Convert a panic that escaped `task.exec`/`task.next` into the ordinary
    /// task-error path. The recovery work itself is isolated as well, because a
    /// corrupted task may panic again while being reported.
    async fn report_task_panic(
        operation: &'static str,
        task: Arc<Task>,
        ctx: Context,
        result: std::result::Result<(), Box<dyn Any + Send>>,
    ) {
        let Err(err) = result else {
            return;
        };
        let err = Self::panic_payload_error(err);
        error!(operation, error = %err, "scheduler operation panicked");
        let report_result = CatchPanic::new(async move {
            task.set_err(&err.clone().into());
            ctx.set_task(&task);
            ctx.emit_error().await.ok();
        })
        .await;
        if report_result.is_err() {
            error!(operation, "reporting the panicked task panicked");
        }
    }

    fn panic_payload_error(payload: Box<dyn Any + Send>) -> ActError {
        let message = if let Some(message) = payload.downcast_ref::<&str>() {
            (*message).to_string()
        } else if let Some(message) = payload.downcast_ref::<String>() {
            message.clone()
        } else {
            "unknown panic payload".to_string()
        };
        ActError::Runtime(format!("scheduler task panicked: {message}"))
    }

    fn create(config: &Config, store: Option<Arc<dyn KvStore>>) -> crate::Result<Arc<Runtime>> {
        // let scher = Scheduler::new();
        let env = Arc::new(Enviroment::new());
        let cache = Arc::new(Cache::new(config, store)?);
        let process_gate = ProcessGate::new(config.scheduler_workers());
        let emitter = Arc::new(Emitter::with_process_gate(process_gate));
        let package = Arc::new(Package::new());
        let queue = Queue::new(config.scheduler_queue_cap());
        let shutdown = CancellationToken::new();
        let schema_cache = Arc::new(SchemaCache::new());
        let snapshots = Arc::new(SnapshotRegistry::new());
        let runtime = Arc::new(Runtime {
            config: Arc::new(config.clone()),
            emitter,
            // scher,
            queue,
            env,
            cache,
            package,
            shutdown,
            schema_cache,
            snapshots,
        });

        runtime.initialize()?;
        Ok(runtime)
    }

    fn initialize(self: &Arc<Self>) -> crate::Result<()> {
        {
            let cache = self.cache.clone();
            let rt = self.clone();
            self.emitter.on_proc(move |proc| {
                let cache = cache.clone();
                let rt = rt.clone();
                async move {
                debug!(pid = %proc.id(), "proc event");
                if let Some(root) = proc.root() {
                    let state = proc.state();
                    let mut message = root.create_message();
                    if state.is_running() || state.is_pending() {
                        let emitter = rt.emitter().clone();
                        emitter.emit_start_event(&message);
                    } else {
                        if state.is_error() {
                            let emitter = rt.emitter().clone();
                            let message = message.clone();
                            emitter.emit_error(&message);
                        } else if state.is_completed() {
                            let mut is_validation_err = false;
                            let model = proc.model();
                            let exposes = &model.exposes;
                            if !exposes.is_empty() {
                                // validate the process outputs
                                let schema = crate::ActSchema::Multiple(exposes.clone());
                                if let Err(e) = schema
                                    .validate(&(message.outputs.to_value()))
                                    .map_err(|err| {
                                        ActError::Model(format!(
                                            "model({}) outputs validation error: {}",
                                            model.id,
                                            err
                                        ))
                                    })
                                {
                                    is_validation_err = true;
                                    let error = e.to_string();
                                    message.set_err("", &error);
                                    proc.set_err(&Error::new(&error, ""));
                                    let emitter = rt.emitter().clone();
                                emitter.emit_error(&message);
                                }
                            }

                            if !is_validation_err {
                                let emitter = rt.emitter().clone();
                                emitter.emit_complete_event(&message);
                            }
                        }
                        let final_state = proc.state();
                        if final_state.is_error() {
                            info!(pid = %proc.id(), state = %final_state, cost_ms = proc.cost(), "process errored");
                        } else if final_state.is_completed() {
                            info!(pid = %proc.id(), state = %final_state, cost_ms = proc.cost(), "process completed");
                        }

                        // if the process is a sub process
                        // call the parent act
                        if let Some((ppid, ptid)) = proc.parent() {
                            rt.return_to_act(&ppid, &ptid, &proc).await;
                        }

                        // Finished: evict the process from the in-memory cache
                        // right away — its slot is freed so `restore` can
                        // start a parked process into it. The durable rows are
                        // NOT deleted here: they are removed by the sweeper
                        // only after every delivery of the process's messages
                        // settled (see
                        // `Store::mark_removable` / `sweep_settled_procs`) —
                        // delivery completion lags the terminal state, so
                        // deleting now would race the still-in-flight
                        // deliveries.

                        cache.evict(proc.id());
                        let rt = rt.clone();
                        // the freed slot first resumes queued in-flight rows
                        // (boot overflow), then refills parked (`None`) rows
                        if let Err(err) = rt.restore().await {
                            error!(error = %err, "process restore failed");
                        }
                    }
                } else {
                    error!(pid = %proc.id(), "cannot find root task");
                }
                }
            });
        }
        {
            let cache = self.cache.clone();
            let rt = self.clone();
            self.emitter.on_task(move |e| {
                let cache = cache.clone();
                let rt = rt.clone();
                async move {
                    debug!(pid = %e.inner().pid, tid = %e.inner().id, "task event");
                    let cache = cache.clone();
                    let e_clone = e.clone();
                    cache
                        .upsert_async(&e_clone)
                        .await
                        .unwrap_or_else(|err| error!(error = %err, "task upsert failed"));

                    // check task is allowed to emit message to client
                    if !e.state().is_pending() && !e.state().is_running() && e.is_emit() {
                        let msg = e.create_message();
                        debug!(pid = %msg.pid, tid = %msg.tid, name = %msg.name, "emit message");
                        let emitter = rt.emitter().clone();
                        emitter.emit_message(&msg);
                    }
                }
            });
        }

        Ok(())
    }

    pub fn init_retry_timer(self: &Arc<Self>) -> crate::Result<()> {
        // Message retry timer — periodically re-send unacknowledged messages
        let max_message_retry_times = self.config().max_message_retry_times();
        #[cfg(not(test))]
        let interval_ms = {
            let secs = if self.config().tick_interval_secs() > 0 {
                self.config().tick_interval_secs()
            } else {
                15
            };
            (secs * 1000) as u64
        };
        #[cfg(test)]
        let interval_ms = 800u64;

        let evt = self.emitter().clone();
        let cache = self.cache.clone();
        let rt = self.clone();
        let shutdown = self.shutdown.clone();
        Handle::current().spawn(async move {
            let mut intv = time::interval(Duration::from_millis(interval_ms));
            loop {
                tokio::select! {
                    _= shutdown.cancelled() => break,
                    _ = intv.tick() => {}
                }
                // each not-yet-acked delivery row is re-sent to the channel it
                // belongs to only
                match cache
                    .store()
                    .with_no_response_deliveries(interval_ms as i64, max_message_retry_times)
                    .await
                {
                    Ok(rearmed) => {
                        for d in rearmed {
                            let store = cache.store();
                            match store.messages().find(&d.msg_id).await {
                                Ok(message) => {
                                    let emitter = evt.clone();
                                    let mut msg: crate::event::Message = message.into();
                                    msg.delivery_id = Some(d.id.clone());
                                    emitter.emit_delivery(&d.chan_id, &msg);
                                }
                                Err(err) => {
                                    // orphan delivery: its canonical message
                                    // is gone, it can never be re-sent — drop it
                                    error!(delivery_id = %d.id, msg_id = %d.msg_id, error = %err, "delivery without canonical message dropped");
                                    if let Err(e) = store.deliveries().delete(&d.id).await {
                                        error!(error = %e, "orphan delivery delete failed");
                                    }
                                }
                            }
                        }
                    }
                    Err(err) => error!(error = %err, "no-response deliveries query failed"),
                }

                // delete finished processes whose deliveries have all settled
                // (the proc completion itself never deletes rows — it waits
                // for the deliveries that lag behind)
                if let Err(err) = cache.sweep_removable().await {
                    error!(error = %err, "settled-process sweep failed");
                }

                // Replay durable scheduler overflow after the in-memory queue
                // has had one full tick to drain.
                if let Err(err) = rt.recover_overflow((interval_ms * 2) as i64).await {
                    error!(error = %err, "scheduler overflow recovery failed");
                }
            }
        });

        Ok(())
    }

    async fn return_to_act(self: &Arc<Self>, pid: &str, tid: &str, proc: &Process) {
        debug!(pid = %pid, tid = %tid, "return to act");
        let state = proc.state();
        // process.print();
        let mut vars = proc.outputs();
        debug!(pid = %pid, tid = %tid, outputs = %vars, "sub outputs");

        let event = match state {
            TaskState::Aborted => EventAction::Abort,
            TaskState::Skipped => EventAction::Skip,
            TaskState::Error => {
                if let Some(err) = proc.err() {
                    vars.set(consts::ACT_ERR_CODE, err.ecode);
                    vars.set(consts::ACT_ERR_MESSAGE, err.message);
                }

                EventAction::Error
            }
            _ => EventAction::Next,
        };

        let action = Action::new(pid, tid, event, vars);
        let scher = self.clone();
        if let Err(err) = scher.do_action(&action).await {
            error!(error = %err, "return to act failed");
        }
    }
    /// Schedule-trigger timer — periodically fires every due `schedule`
    /// trigger row and rolls its `next_run` forward. Deployed rows arm with
    /// their next cron fire; a changed schedule re-arms the same way.
    pub fn init_trigger_timer(self: &Arc<Self>) {
        #[cfg(not(test))]
        let interval_ms = {
            let secs = self.config().tick_interval_secs();
            if secs > 0 {
                (secs * 1000) as u64
            } else {
                15_000
            }
        };
        #[cfg(test)]
        let interval_ms = 800u64;

        let store = self.store();
        let shutdown = self.shutdown.clone();
        let rt = self.clone();
        tokio::spawn(async move {
            let mut intv = time::interval(Duration::from_millis(interval_ms));
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = intv.tick() => {}
                }
                let now = crate::utils::time::time_millis();
                let due = match store
                    .events()
                    .query(
                        &crate::query::Query::new().limit(1000).filter(
                            crate::query::Filter::and()
                                .expr(crate::query::Expr::eq("kind", "schedule"))
                                .expr(crate::query::Expr::le("next_run", now)),
                        ),
                    )
                    .await
                {
                    Ok(rows) => rows.rows,
                    Err(err) => {
                        error!(error = %err, "schedule query failed");
                        continue;
                    }
                };
                for event in due {
                    if let Err(err) = rt.fire_schedule(&event).await {
                        error!(event = %event.id, error = %err, "schedule trigger failed");
                    }
                }
            }
        });
    }

    /// Snapshot TTL sweep — periodically drops expired cache entries so
    /// never-read, never-tombstoned scopes cannot grow the cache forever.
    pub fn init_snapshot_timer(self: &Arc<Self>) {
        #[cfg(not(test))]
        let interval_ms = {
            let secs = self.config().tick_interval_secs();
            if secs > 0 {
                (secs * 1000) as u64
            } else {
                15_000
            }
        };
        #[cfg(test)]
        let interval_ms = 800u64;

        let registry = self.snapshot_registry();
        let shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            let mut intv = time::interval(Duration::from_millis(interval_ms));
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = intv.tick() => {}
                }
                let removed = registry.purge_expired();
                if removed > 0 {
                    debug!(removed, "expired snapshot entries purged");
                }
            }
        });
    }

    /// fire one due schedule trigger: start the workflow with the trigger's
    /// default params and roll `last_run`/`next_run` forward. The row state
    /// is persisted after the start, so a crash between start and state roll
    /// may re-fire the trigger on recovery (at-least-once).
    async fn fire_schedule(self: &Arc<Self>, event: &data::Event) -> Result<()> {
        let model = self.cache.store().models().find(&event.mid).await?;
        let model: crate::ModelInfo = model.into();
        let workflow = model.workflow()?;

        let payload = event.default_params();
        let inputs = match payload {
            serde_json::Value::Null => Vars::new(),
            value => serde_json::from_value::<Vars>(value)
                .map_err(|e| ActError::Convert(format!("invalid trigger payload: {e}")))?,
        };
        let started = self.start(&workflow, inputs).await;

        // roll the schedule forward even when the start failed, so a failing
        // trigger does not hot-loop on every tick; the error is logged by the
        // caller (at-least-once delivery)
        let mut event = event.clone();
        event.last_run = crate::utils::time::time_millis();
        event.next_run = match event.schedule.as_deref() {
            Some(schedule) => super::cron::Cron::next_fire_millis(schedule),
            None => 0,
        };
        self.cache.store().events().update(&event).await?;
        started.map(|_| ())
    }
}
