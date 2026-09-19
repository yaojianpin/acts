use super::health::{LoopGuard, SchedulerHealth};
use super::ops::{OpClaim, OpClaims, OpKey, has_live_descendant};
use super::validation::SchemaCache;
use super::{ActTask, Context, Process, Task, TaskState};
use crate::snapshot::{SnapshotOptions, SnapshotStore};
use crate::{
    ActError, Action, Config, Error, Package, Result, ShareLock, Vars, Workflow,
    cache::Cache,
    data,
    env::Environment,
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
    time::{Duration, Instant},
};
use tokio::{runtime::Handle, time};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, instrument, warn};

#[derive(Clone)]
pub struct Runtime {
    config: Arc<Config>,
    queue: Arc<Queue>,
    env: Arc<Environment>,
    cache: Arc<Cache>,
    emitter: Arc<Emitter>,
    package: Arc<Package>,
    shutdown: CancellationToken,
    schema_cache: Arc<SchemaCache>,
    pub(crate) snapshots: Arc<SnapshotRegistry>,
    /// Failure state of the schedule-trigger timer (see [`SchedulerHealth`]).
    trigger_health: Arc<LoopGuard>,
    /// Failure state of the message-retry timer (see [`SchedulerHealth`]).
    retry_health: Arc<LoopGuard>,
    /// The propagation operations this engine's jobs own, and the ones a job
    /// left behind (see [`crate::scheduler::ops`]).
    op_claims: Arc<OpClaims>,
    /// How long a stalled operation waits before its first liveness re-drive;
    /// the window doubles per attempt from here (one timer tick, so a stall is
    /// noticed within a tick of the job that abandoned it).
    stall_backoff: Duration,
}

/// Tick the periodic timers run on under test — short enough that a test can
/// watch several ticks, and the one source of truth for tests that reason in
/// ticks (see `scheduler::tests`).
#[cfg(test)]
pub(crate) const TEST_TICK_MS: u64 = 800;

/// How many liveness re-drives a stalled `next` propagation gets before its
/// condition is reported as a warning: the pass keeps retrying on a widening
/// window, so this is the point at which "it has not converged" is worth
/// saying out loud.
const STALLED_OP_WARN_ATTEMPTS: u32 = 4;

/// Registry of snapshot-backed sealed-data targets (see [`crate::snapshot`]).
pub(crate) struct SnapshotRegistry {
    stores: ShareLock<HashMap<String, Arc<SnapshotStore>>>,
}

/// The two kinds of work a lane worker executes. The name is what a caught
/// panic is reported under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobOp {
    Exec,
    Next,
    Error,
    Abort,
}

impl JobOp {
    fn as_str(self) -> &'static str {
        match self {
            JobOp::Exec => "task.exec",
            JobOp::Next => "task.next",
            JobOp::Error => "propagation.error",
            JobOp::Abort => "propagation.abort",
        }
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
            .field("health", &self.scheduler_health())
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
    pub fn env(&self) -> &Arc<Environment> {
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

    /// The propagation operations this engine's jobs own, and the ones a job
    /// left behind (see [`crate::scheduler::ops`]).
    pub(crate) fn op_claims(&self) -> &Arc<OpClaims> {
        &self.op_claims
    }

    /// How long an operation abandoned by its job waits before the liveness
    /// pass re-drives it.
    pub(crate) fn stall_backoff(&self) -> Duration {
        self.stall_backoff
    }

    /// Scheduler-backlog metrics: the configured bound, the per-lane bound, the
    /// number of jobs currently buffered in the lanes, and the high watermark of
    /// that depth. The backlog is real: a lane that is full refuses its
    /// producers, so `depth` can never exceed the effective bound (the lane
    /// count × the per-lane bound) — the work beyond it is in the durable
    /// outbox, visible through pending ops.
    pub fn scheduler_queue_capacity(&self) -> usize {
        self.queue.capacity()
    }

    /// Per-lane bound: how much work one process can have buffered at once
    /// (`scheduler_queue_cap` split across the lanes).
    pub fn scheduler_lane_capacity(&self) -> usize {
        self.queue.lane_capacity()
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

    /// Whether the store write path is saturated and therefore refusing new
    /// work (a task's state write, or the outbox record standing in for it)
    /// until its backlog drains.
    pub fn store_writer_saturated(&self) -> bool {
        self.cache.store_writer_saturated()
    }

    /// Failure state of the store-facing background timers: consecutive failed
    /// ticks and the backoff window each is on (see [`SchedulerHealth`]). A
    /// loop whose store keeps failing is reported `Degraded` and skips ticks
    /// instead of polling the store at the full tick rate, so this is where a
    /// host reads *why* scheduled triggers are late or deliveries are not
    /// being re-sent.
    pub fn scheduler_health(&self) -> SchedulerHealth {
        SchedulerHealth {
            trigger: self.trigger_health.snapshot(),
            retry: self.retry_health.snapshot(),
        }
    }

    #[allow(unused)]
    pub fn config(&self) -> &Arc<Config> {
        &self.config
    }
    pub(crate) fn register_snapshot(
        &self,
        name: &str,
        options: SnapshotOptions,
    ) -> Result<Arc<SnapshotStore>> {
        options.validate()?;
        Ok(self.snapshots.register(name, options))
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
    pub async fn start(
        self: &Arc<Self>,
        model: &Workflow,
        mut options: Vars,
    ) -> Result<Arc<Process>> {
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

        // The caller's authority travels inside the start options (sealed by
        // `Executor`), never as a model input: it is popped before anything
        // else reads the options, so it can neither fail an input schema nor
        // leak into the workflow's user vars.
        let owner = options.pop::<crate::ScopePolicy>(consts::PROC_OWNER);

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
        if let Some(owner) = owner {
            // The workdir root travels with the owner authority, never as a
            // start option: it is compiled from the config's ACL, so a caller
            // can neither name the directory nor place a run outside the one
            // its policy confines it to. The directory lives exactly as long
            // as the process's durable rows — the sweeper removes it with
            // them, and a start that never became durable removes it itself
            // (see `Cache::abandon`).
            if let Some(root) = owner.workdir_root.as_deref() {
                let dir = prepare_workdir(root, &proc_id)?;
                proc.set_workdir(&dir);
            }
            proc.set_owner_scope(&owner);
        }

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
            // A start that failed must give its pid back (`Cache::abandon`):
            // the claim guards an admission that is becoming durable, so a pid
            // whose start never reached the store can be started again — while
            // a retained claim would fail every later start of the same
            // external pid as a duplicate although nothing is running. The
            // workdir the start just created goes with the pid: with no row,
            // no sweep would ever find it.
            self.cache.abandon(&proc).await;
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
    /// thanks to the durably persisted applied propagation phase.
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
    /// the task persist (with the applied propagation phase) and then the outbox
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
    ///   by the durable applied propagation guard and closed;
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
                    // close was lost) — close and settle the engine-owned
                    self.cache.store().complete_ops(&pid, &tid, &r#type).await?;
                    self.cache.store().close_deliveries(&pid, &tid).await?;
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
                if task.state().is_completed() {
                    self.cache.store().complete_ops(&pid, &tid, &r#type).await?;
                } else {
                    match self.queue.send(&task) {
                        Ok(()) => {
                            if let Err(err) = self.cache.mark_op_dispatched(&op).await {
                                error!(error = %err, pid = %pid, tid = %tid, "failed to mark replayed exec dispatched");
                            }
                        }
                        // Still full: retain the descriptor for the periodic
                        // overflow consumer.
                        Err(ActError::QueueFull) => continue,
                        Err(err) => return Err(err),
                    }
                }
            } else if r#type == data::OpType::Error.as_ref() {
                match self.queue.send_error(&task) {
                    Ok(()) => {
                        if let Err(err) = self.cache.mark_op_dispatched(&op).await {
                            error!(error = %err, pid = %pid, tid = %tid, "failed to mark replayed error dispatched");
                        }
                    }
                    Err(ActError::QueueFull) => continue,
                    Err(err) => return Err(err),
                }
            } else if r#type == data::OpType::Abort.as_ref() {
                match self.queue.send_abort(&task) {
                    Ok(()) => {
                        if let Err(err) = self.cache.mark_op_dispatched(&op).await {
                            error!(error = %err, pid = %pid, tid = %tid, "failed to mark replayed abort dispatched");
                        }
                    }
                    Err(ActError::QueueFull) => continue,
                    Err(err) => return Err(err),
                }
            } else if task.is_propagation_done() {
                // propagation already completed durably; just close the record
                // and settle the engine-owned deliveries (an `Error` row stays).
                // A durable `applied` marker beside a *non-terminal* state is
                // not that proof (see `Task::is_propagation_done`): it falls
                // through to the replay below, which is what finishes the
                // propagation the cut interrupted.
                self.cache.store().complete_ops(&pid, &tid, &r#type).await?;
                self.cache.store().close_deliveries(&pid, &tid).await?;
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
    ///
    /// The same pass is the **liveness net for `next` propagation**: a record
    /// whose job ended without closing it (a panic, a cancelled future, a queue
    /// that refused the dispatch, a close whose write was lost) has no other
    /// runtime re-drive — the boot replay only runs at startup — so the record
    /// stays open, its task's parent waits for a propagation that will never
    /// come, and the process holds a resident slot forever. Re-driving is
    /// guarded on all four sides:
    ///
    /// - the in-memory claim of the operation ([`OpClaims`]): a record a job is
    ///   running *right now* is never replayed;
    /// - the task's propagation phase: a record whose propagation is already
    ///   durable is closed instead of replayed — the effect happened, only the
    ///   close was lost;
    /// - the task's own state ([`Task::next_is_drivable`]): only a *running*
    ///   auto-completing task is re-driven. A terminal task's record is closed,
    ///   never replayed (a re-run would schedule work a cancel/back/remove/abort
    ///   decision already resolved), and a task waiting on the outside world —
    ///   a client action, a sibling branch, its first run, a subflow's child
    ///   process — owns its open record by design;
    /// - the task's subtree: while a descendant is still in flight, its
    ///   completion recurses into this task's `next` and closes the record, so
    ///   the pass leaves it alone (this is what stops a legitimately waiting
    ///   parent from being re-driven every tick);
    /// - the row's own identity (`source_version`/`target_tid`): a record
    ///   written for a superseded generation of the same task slot is closed,
    ///   never replayed onto the current one.
    ///
    /// A re-drive is idempotent because the operation itself is: `Task::next`
    /// starts from the durable applied-propagation guard, `schedule_once`
    /// reuses the task instance a replay would have created, and the task state
    /// transitions are guarded (`set_state_if_running`). The pass retries on a
    /// doubling window (see [`OpClaims::attempted`]), so a record that cannot
    /// make progress is reported instead of hammering the store.
    pub(crate) async fn recover_outbox(self: &Arc<Self>, older_than_millis: i64) -> Result<()> {
        let store = self.cache.store();
        let now = Instant::now();
        // Claims of processes the engine no longer holds: their rows are swept
        // with them, so there is nothing left to re-drive.
        self.op_claims
            .retain_processes(&|pid| self.cache.resident(pid).is_some());
        for op in store.load_scheduled_ops(older_than_millis).await? {
            let r#type = op.r#type.clone();
            let (pid, tid) = (op.pid.clone(), op.tid.clone());
            let is_exec_overflow = r#type == data::OpType::Exec.as_ref()
                && op.status == data::OpStatus::Pending.as_ref();
            let is_next = r#type == data::OpType::Next.as_ref();
            let is_error_pending = r#type == data::OpType::Error.as_ref()
                && op.status == data::OpStatus::Pending.as_ref();
            let is_abort_pending = r#type == data::OpType::Abort.as_ref()
                && op.status == data::OpStatus::Pending.as_ref();
            if !is_exec_overflow && !is_next && !is_error_pending && !is_abort_pending {
                continue;
            }

            let Some(proc) = self.cache.proc(&pid, self).await? else {
                store.complete_ops(&pid, &tid, &r#type).await?;
                continue;
            };
            let Some(task) = proc.task(&tid) else {
                store.complete_ops(&pid, &tid, &r#type).await?;
                continue;
            };

            if is_next {
                let key = OpKey::new(&pid, &tid, data::OpType::Next);
                // a job is running this operation right now: the row belongs
                // to it, and its own close (or its own release) is what moves
                // the record on
                if self.op_claims.is_running(&key) {
                    continue;
                }
                if task.is_propagation_done() {
                    // the propagation is durable and only its close was lost
                    store.complete_ops(&pid, &tid, &r#type).await?;
                    store.close_deliveries(&pid, &tid).await?;
                    self.op_claims.clear(&key);
                    continue;
                }
                // a descendant's completion re-enters this task's `next`,
                // which is what closes this record; and a task waiting on the
                // outside world (a client action, a sibling branch, its first
                // run, a subflow's child process) owns an open record by
                // design, so neither is a re-drive target
                if has_live_descendant(&task) || !task.next_is_drivable() {
                    continue;
                }
                if !self.op_claims.may_retry(&key, now) {
                    continue;
                }
                // the record must describe *this* task's propagation: a row
                // written for a superseded generation of the same slot has
                // nothing left to do
                if op.source_version != task.timestamp || op.target_tid != task.parent_id() {
                    store.complete_ops(&pid, &tid, &r#type).await?;
                    continue;
                }
                match self.queue.send_next(&task) {
                    Ok(()) => {
                        if let Err(err) = self.cache.mark_op_dispatched(&op).await {
                            error!(error = %err, pid = %pid, tid = %tid, "failed to mark re-driven next dispatched");
                        }
                    }
                    // Still full: the record becomes the disk queue's work.
                    Err(ActError::QueueFull) => {
                        if let Err(err) = self.cache.mark_next_overflow(&task).await {
                            error!(error = %err, pid = %pid, tid = %tid, "failed to mark re-driven next overflow");
                        }
                    }
                    Err(err) => {
                        error!(error = %err, pid = %pid, tid = %tid, "cannot re-drive the stalled next propagation");
                    }
                }
                let attempts = self.op_claims.attempted(&key, self.stall_backoff);
                if attempts == STALLED_OP_WARN_ATTEMPTS {
                    warn!(pid = %pid, tid = %tid, attempts, "a next propagation cannot make progress: its task waits on a child that is already terminal without a finished propagation, or on a store that keeps refusing its state write");
                }
                continue;
            }

            let is_propagation =
                r#type == data::OpType::Error.as_ref() || r#type == data::OpType::Abort.as_ref();
            let propagation_applied =
                is_propagation && data::OpPhase::from_task_value(op.phase.as_str()).is_applied();
            if task.state().is_completed() && !propagation_applied {
                store.complete_ops(&pid, &tid, &r#type).await?;
                continue;
            }
            // a hop a job owns right now is not re-driven: the row belongs to
            // that job, which closes it after its effect (`next` records are
            // guarded the same way above)
            if is_propagation {
                let hop = if r#type == data::OpType::Error.as_ref() {
                    data::OpType::Error
                } else {
                    data::OpType::Abort
                };
                if self.op_claims.is_running(&OpKey::new(&pid, &tid, hop)) {
                    continue;
                }
            }

            let queued = if r#type == data::OpType::Error.as_ref() {
                self.queue.send_error(&task)
            } else if r#type == data::OpType::Abort.as_ref() {
                self.queue.send_abort(&task)
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

    /// Resident-set refill: a slot just freed (a process finished and its
    /// terminal event evicted it), so load queued in-flight rows (boot-resume
    /// overflow that did not fit the cap) into the free slots and re-dispatch
    /// them, then refill parked (`None`) rows — non-`None` first, matching
    /// boot priority.
    ///
    /// Called on a terminal event, and on the periodic tick as well: a process
    /// the *sweeper* deletes from the store also frees its slot (its rows were
    /// swept, so no terminal event follows), and without the tick's call the
    /// refill would wait for a trigger that never comes — the parked processes
    /// behind it would never start.
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

    /// Start the fixed lane workers. Every lane owns one bounded queue and runs
    /// its jobs serially, so all work of one process stays FIFO on that
    /// process's lane while independent processes overlap on the other lanes.
    /// The lane count is the explicit in-flight limit (jobs executing at once);
    /// the lanes' bounds are the in-memory backlog limit — a lane that is full
    /// refuses its producer, which durably queues the work instead of letting a
    /// slow lane absorb an unbounded amount of it.
    pub fn event_loop(self: &Arc<Self>) {
        let queue = self.queue.clone();
        let gate = self.emitter.process_gate();
        let shutdown = self.shutdown.clone();
        let mut workers = Vec::with_capacity(queue.lanes());
        for mut receiver in queue.take_receivers() {
            let gate = gate.clone();
            let shutdown = shutdown.clone();
            workers.push(tokio::spawn(async move {
                loop {
                    let data = tokio::select! {
                        _ = shutdown.cancelled() => break,
                        data = receiver.recv() => match data {
                            Some(data) => data,
                            // every sender is gone: nothing can be admitted again
                            None => break,
                        },
                    };
                    let (task, proc, operation) = match data {
                        QueueData::Task { task, proc } => (task, proc, JobOp::Exec),
                        QueueData::Next { task, proc } => (task, proc, JobOp::Next),
                        QueueData::Error { task, proc } => (task, proc, JobOp::Error),
                        QueueData::AbortPropagation { task, proc } => (task, proc, JobOp::Abort),
                        QueueData::Abort => break,
                    };
                    // Serialization: the lane already orders one process's jobs,
                    // and the gate (same pid hash) keeps the pid's workflow
                    // event handlers from interleaving with them.
                    let _gate = gate.lock(&task.pid).await;
                    Runtime::execute_job(task, proc, operation).await;
                }
            }));
        }
        tokio::spawn(async move {
            // The lease lives exactly as long as the pool: producers that
            // outlive every worker are refused instead of buffering work
            // nothing will run. A lane whose worker died refuses on its own —
            // its channel is closed and `try_push` reports that to the producer.
            let _consumer = queue.consumer_lease();
            for (lane, worker) in workers.into_iter().enumerate() {
                if let Err(err) = worker.await {
                    error!(lane, error = %err, "scheduler lane worker exited");
                }
            }
        });
    }

    /// Build an execution context while catching a panic at the poll boundary.
    async fn execute_job(task: Arc<Task>, proc: Arc<Process>, operation: JobOp) {
        // The operation has left the queue. Record that boundary before the
        // effect starts; a failed phase note is observability damage, not a
        // reason to drop already-accepted work.
        let op_type = match operation {
            JobOp::Next => Some(data::OpType::Next),
            JobOp::Error => Some(data::OpType::Error),
            JobOp::Abort => Some(data::OpType::Abort),
            JobOp::Exec => None,
        };
        // Claim the operation for as long as this job runs it. The guard is
        // installed even when the phase note fails: what makes a stranded
        // record findable is knowing that *no* job owns it — a claim released
        // by the drop below is that knowledge, on every exit path (a return,
        // a caught panic, a dropped future), and the liveness pass re-drives
        // what no job closed.
        let mut claim = op_type.map(|op_type| {
            OpClaim::start(
                task.runtime().op_claims(),
                &task.pid,
                &task.id,
                op_type,
                task.runtime().stall_backoff(),
            )
        });
        if let Some(op_type) = op_type
            && let Err(err) = task
                .runtime()
                .cache()
                .mark_op_phase(&task.pid, &task.id, op_type, data::OpPhase::EffectInFlight)
                .await
        {
            error!(error = %err, "failed to mark operation effect-in-flight");
        }
        let Some(ctx) = Self::isolate_context(task.clone(), proc).await else {
            return;
        };
        let reporting_task = task.clone();
        let reporting_ctx = ctx.clone();
        let result = CatchPanic::new(async move {
            match operation {
                JobOp::Exec => {
                    // never execute work for a process that is already over:
                    // a terminal walk can race a queued dispatch, and running
                    // it would grow the tree under a dead run
                    if task.proc().is_some_and(|p| p.state().is_completed()) {
                        return true;
                    }
                    Self::run_exec_job(task, ctx).await;
                    true
                }
                JobOp::Next => Self::run_next_job(task, ctx).await,
                JobOp::Error => {
                    Self::run_error_job(task, ctx).await;
                    true
                }
                JobOp::Abort => {
                    Self::run_abort_job(task, ctx).await;
                    true
                }
            }
        })
        .await;
        let closed =
            Self::report_task_panic(operation.as_str(), reporting_task, reporting_ctx, result)
                .await
                .unwrap_or(false);
        // A panic is `Err` here, and the operation it interrupted did not
        // close anything: the claim is released as stalled, and the liveness
        // pass decides whether anything else can finish it. The Error/Abort
        // runners reach their record close on their own path, and `next`
        // reports whether it closed its record, so `Some(true)` means "the
        // effect ran to its end" — what the guard needs to know.
        if let Some(claim) = claim.as_mut()
            && closed
        {
            claim.close();
        }
    }

    async fn run_exec_job(task: Arc<Task>, ctx: Context) {
        if let Err(err) = task.exec(&ctx).await {
            // An action applied while the act ran (`abort`, `cancel`, `skip`,
            // `remove`, `next`, an external `error`) already decided this
            // task's outcome. An act that noticed — its cancellation token
            // fires — and returned an error reports the consequence of that
            // decision, not a new one, and must not overwrite it: the state a
            // workflow asked for has to stick.
            if task.state().is_completed() {
                debug!(error = %err, "task was overridden while it ran; its act's error is ignored");
                return;
            }
            error!(error = %err, "task.exec failed");
            task.set_err(&err.clone().into());
            ctx.set_task(&task);
            ctx.emit_error().await.ok();
        }
    }

    /// Run a task's `next` propagation and report whether it *closed* the
    /// task's durable outbox record.
    ///
    /// `false` means the record is still open when this job ends. That is a
    /// legitimate outcome — children in flight, an interrupt, a step that has
    /// not finished counting its children — and it is what the claim guard
    /// hands to the liveness pass: the record stays open, and whether anything
    /// else re-enters the task (a descendant's completion) decides if the pass
    /// has to.
    async fn run_next_job(task: Arc<Task>, ctx: Context) -> bool {
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
            return true;
        }
        // On success the record is closed inside `next` once the task reaches
        // a terminal state (the applied propagation marker is that close's
        // in-memory witness); outcomes with children still in flight or an
        // interrupt leave it open.
        task.is_propagation_applied()
    }

    /// Run an Error propagation descriptor. `on_error` may schedule a catch
    /// branch and handle the error locally, or continue through the established
    /// parent handler; the outbox record is closed only after this returns.
    async fn run_error_job(task: Arc<Task>, ctx: Context) {
        let result = task.on_error(&ctx).await;
        if let Err(err) = result {
            error!(error = %err, "error propagation failed");
        }
        if let Err(err) = task
            .runtime()
            .cache()
            .complete_propagation(&task, data::OpType::Error)
            .await
        {
            error!(error = %err, "complete error propagation failed");
        }
    }

    /// Run one hop of abort propagation. The target is already terminal in the
    /// normal path; this job settles its non-terminal descendants and queues
    /// the parent hop, rather than walking the ancestor chain inline.
    async fn run_abort_job(task: Arc<Task>, ctx: Context) {
        let result = ctx.abort_one_hop(&task).await;
        if let Err(err) = result {
            error!(error = %err, "abort propagation failed");
        }
        if let Err(err) = task
            .runtime()
            .cache()
            .complete_propagation(&task, data::OpType::Abort)
            .await
        {
            error!(error = %err, "complete abort propagation failed");
        }
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
    ///
    /// Returns what the operation reported about its outbox record (`None` when
    /// it panicked — the record is left exactly as it is, for the claim guard
    /// to hand to the liveness pass).
    async fn report_task_panic(
        operation: &'static str,
        task: Arc<Task>,
        ctx: Context,
        result: std::result::Result<bool, Box<dyn Any + Send>>,
    ) -> Option<bool> {
        let payload = match result {
            Ok(closed) => return Some(closed),
            Err(payload) => payload,
        };
        let err = Self::panic_payload_error(payload);
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
        None
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
        let env = Arc::new(Environment::new());
        let cache = Arc::new(Cache::new(config, store)?);
        let process_gate = ProcessGate::new(config.scheduler_workers());
        let emitter = Arc::new(Emitter::with_process_gate(process_gate.clone()));
        let package = Arc::new(Package::new());
        // The same gate routes jobs: a lane is the pid hash both the queue and
        // the event handlers use, so the two can never disagree.
        let queue = Queue::new(config.scheduler_queue_cap(), process_gate);
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
            trigger_health: LoopGuard::new("schedule-trigger"),
            retry_health: LoopGuard::new("message-retry"),
            op_claims: Arc::new(OpClaims::new()),
            stall_backoff: Self::tick_interval(config),
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

    /// The periodic timers' interval: the configured tick, or the engine's
    /// default of 15s, and [`TEST_TICK_MS`] under test. One rule for every
    /// timer that reasons in ticks (and for how long an abandoned outbox
    /// operation waits before its first liveness re-drive).
    fn tick_interval(config: &Config) -> Duration {
        #[cfg(test)]
        {
            let _ = config;
            Duration::from_millis(TEST_TICK_MS)
        }
        #[cfg(not(test))]
        {
            let secs = config.tick_interval_secs();
            let secs = if secs > 0 { secs } else { 15 };
            Duration::from_millis((secs * 1000) as u64)
        }
    }

    pub fn init_retry_timer(self: &Arc<Self>) -> crate::Result<()> {
        // Message retry timer — periodically re-send unacknowledged messages
        let max_message_retry_times = self.config().max_message_retry_times();
        let interval_ms = Self::tick_interval(self.config()).as_millis() as u64;

        let evt = self.emitter().clone();
        let cache = self.cache.clone();
        let rt = self.clone();
        let shutdown = self.shutdown.clone();
        let health = self.retry_health.clone();
        Handle::current().spawn(async move {
            let mut intv = time::interval(Duration::from_millis(interval_ms));
            loop {
                tokio::select! {
                    _= shutdown.cancelled() => break,
                    _ = intv.tick() => {}
                }
                // A degraded loop sits this tick out: the store has failed
                // every recent attempt, so another query now would fail too
                // (and log again) — the guard decides when the next attempt is
                // worth making.
                if !health.attempt() {
                    continue;
                }
                // One tick is one attempt: every store call below runs, and the
                // tick counts as failed if any of them did — a store that
                // cannot serve one of them cannot serve the next tick either.
                let mut failure: Option<ActError> = None;

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
                    Err(err) => {
                        error!(error = %err, "no-response deliveries query failed");
                        failure = Some(err);
                    }
                }

                // delete finished processes whose deliveries have all settled
                // (the proc completion itself never deletes rows — it waits
                // for the deliveries that lag behind)
                if let Err(err) = cache.sweep_removable().await {
                    error!(error = %err, "settled-process sweep failed");
                    failure.get_or_insert(err);
                }

                // A swept process released its resident slot without a
                // terminal event of its own (its rows were swept, so nobody
                // else will notice), and a queued in-flight row waits for a
                // slot as well: refilling here is what keeps a parked process
                // from waiting for a trigger that never comes.
                if let Err(err) = rt.restore().await {
                    error!(error = %err, "resident-set refill failed");
                    failure.get_or_insert(err);
                }

                // Hand durable outbox work back to memory after the in-memory
                // queue has had one full tick to drain: the records overflow
                // spilled, and the `next` records whose job ended without
                // closing them.
                if let Err(err) = rt.recover_outbox((interval_ms * 2) as i64).await {
                    error!(error = %err, "scheduler outbox recovery failed");
                    failure.get_or_insert(err);
                }

                match failure {
                    Some(err) => health.failed(&err),
                    None => health.recovered(),
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
        let interval_ms = Self::tick_interval(self.config()).as_millis() as u64;

        let store = self.store();
        let shutdown = self.shutdown.clone();
        let rt = self.clone();
        let health = self.trigger_health.clone();
        tokio::spawn(async move {
            let mut intv = time::interval(Duration::from_millis(interval_ms));
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = intv.tick() => {}
                }
                // A degraded loop sits this tick out: the store has failed
                // every recent attempt, so another query now would fail too
                // (and log again) — the guard decides when the next attempt is
                // worth making.
                if !health.attempt() {
                    continue;
                }
                let now = crate::utils::time::time_millis();
                // The due query is the loop's one store round trip and the only
                // failure that counts against its health: a trigger whose own
                // fire failed is that row's problem, and it repeats only when
                // the row itself is unusable (its model is gone, its payload
                // does not parse) — one such row must not throttle the
                // schedules that are fine. What the health is about is reaching
                // the store, and that is what this query decides.
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
                    Ok(rows) => {
                        health.recovered();
                        rows.rows
                    }
                    Err(err) => {
                        error!(error = %err, "schedule query failed");
                        health.failed(&err);
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
        let interval_ms = Self::tick_interval(self.config()).as_millis() as u64;

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

/// Materialize `<root>/<pid>` — the directory a process's filesystem access is
/// confined to — and return it.
///
/// The process id becomes a path segment here, so it must be one safe
/// component. An externally supplied pid is otherwise free-form (only the key
/// separator is rejected elsewhere, because it was previously only ever a
/// store-key part), and a pid like `../..` would place the process outside the
/// root it was given.
fn prepare_workdir(root: &std::path::Path, pid: &str) -> Result<std::path::PathBuf> {
    if !is_workdir_segment(pid) {
        return Err(ActError::Action(format!(
            "proc id '{pid}' cannot be used as a workdir name: it must be a single path component without '.' or '..'"
        )));
    }

    let dir = root.join(pid);
    std::fs::create_dir_all(&dir).map_err(|err| {
        ActError::Action(format!(
            "failed to create the process workdir {}: {err}",
            dir.display()
        ))
    })?;
    Ok(dir)
}

/// Whether `pid` is usable as a single directory name: non-empty, not `.` or
/// `..`, and containing no path separator, drive/stream colon, NUL or control
/// character. Engine-generated ids ([`crate::utils::longid`]) are alphanumeric
/// and pass unchanged.
fn is_workdir_segment(pid: &str) -> bool {
    !pid.is_empty()
        && pid != "."
        && pid != ".."
        && !pid.contains(['/', '\\', ':', '\0'])
        && !pid.chars().any(char::is_control)
}
