use super::{ActTask, Process, Sign, Task, TaskState};
use crate::snapshot::{SnapshotOptions, SnapshotStore};
use crate::{
    ActError, Action, Config, Error, Package, Result, ShareLock, Vars, Workflow,
    cache::Cache,
    data,
    env::Enviroment,
    event::{Emitter, EventAction},
    scheduler::queue::{Queue, QueueData},
    store::{KvStore, Store},
    utils::{self, consts},
};
use parking_lot::RwLock;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{runtime::Handle, time};
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
    pub(crate) snapshots: Arc<SnapshotRegistry>,
}

/// Registry of snapshot-backed sealed-data targets (see [`crate::snapshot`]).
pub(crate) struct SnapshotRegistry {
    stores: ShareLock<HashMap<String, Arc<SnapshotStore>>>,
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

    pub fn store(&self) -> Arc<Store> {
        self.cache.store().clone()
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

        let mut model = model.clone();
        model.set_vars(&options);

        let proc = Process::new(&proc_id, self);
        proc.load(&model)?;

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
        cache.upsert_async(&task_clone)?;
        self.queue.send(&task_clone)?;
        Ok(())
    }

    /// Dispatch a task to the in-memory queue WITHOUT queueing another store
    /// write — used for the root task of a freshly started process, whose
    /// proc row + root task row were already persisted atomically by
    /// `Cache::start_proc`.
    #[instrument(skip(self, task), fields(pid = %task.pid, tid = %task.id))]
    pub(crate) fn dispatch_root(&self, task: &Arc<Task>) -> Result<()> {
        debug!("root task dispatched");
        self.queue.send(task)?;
        Ok(())
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
    /// is durable first) and the operation is dispatched to the in-memory
    /// queue — neither blocks the caller. A crash before the record lands is
    /// consistent (nothing to replay); a crash after it lands is recovered by
    /// [`Self::recover_actions`]; a crash after the operation ran is a no-op
    /// thanks to the durably persisted `NEXT_COMPLETE` marker.
    pub(crate) fn enqueue_next(&self, task: &Arc<Task>) -> Result<()> {
        self.cache.enqueue_next(task)?;
        self.queue.send_next(task)?;
        Ok(())
    }

    /// Durable outbox close for a task whose `next` propagation finished: queue
    /// the task persist (with the `NEXT_COMPLETE` marker) and then the outbox
    /// record close, in order, on the store writer — non-blocking. Called from
    /// `Task::next` once the task reaches a terminal state (also for the
    /// idempotent replay guard), and from the event loop when `next` ends in
    /// error. Non-terminal outcomes (children in flight, interrupt) leave the
    /// record `Pending` so recovery replays it.
    pub(crate) fn complete_next(&self, task: &Arc<Task>) -> Result<()> {
        self.cache.complete_next(task)
    }

    /// Durable outbox enqueue for a client action (non-`Next` events): the
    /// `Pending` record with the event + options payload is written **before**
    /// the action is applied, so a crash before the task state write lands is
    /// replayed by [`Self::recover_actions`].
    pub(crate) fn enqueue_action(&self, action: &Action) -> Result<()> {
        self.cache.enqueue_action(action)
    }

    /// Durable outbox close for a client action: the state write and message
    /// status were already queued by the caller, so FIFO order makes `Done`
    /// durable only after both.
    pub(crate) fn complete_action(&self, task: &Arc<Task>) -> Result<()> {
        self.cache.complete_action(task)
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
            let (pid, tid) = (op.pid, op.tid);
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
                self.queue.send_next(&task)?;
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
        let shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            loop {
                let next = tokio::select! {
                    _ = shutdown.cancelled() => break,
                    next = queue.next() => next,
                };
                match next {
                    Ok(data) => match data {
                        QueueData::Task { task, proc: _proc } => {
                            // Keep the queued process alive until the item has
                            // fully executed; `_proc` is the execution lease.
                            let ctx = &task.create_context();
                            if let Err(err) = task.exec(ctx).await {
                                error!(error = %err, "task.exec failed");
                                task.set_err(&err.clone().into());
                                ctx.set_task(&task);
                                ctx.emit_error().await.ok();
                            }
                        }
                        QueueData::Next { task, proc: _proc } => {
                            // Same as `Task`: the queue owns an execution lease
                            // across terminal-event eviction races.
                            let ctx = &task.create_context();
                            let result = task.next(ctx).await;
                            if let Err(err) = result {
                                error!(error = %err, "task.next failed");
                                task.set_err(&err.clone().into());
                                ctx.set_task(&task);
                                ctx.emit_error().await.ok();
                                // the propagation ended in error (terminal):
                                // close the outbox record so recovery does not
                                // replay the failed `next`
                                if let Err(err) = task.runtime().complete_next(&task) {
                                    error!(error = %err, "complete_next failed");
                                }
                            }
                            // on success the record is closed inside `next`
                            // once the task reaches a terminal state; outcomes
                            // with children still in flight or an interrupt
                            // leave it `Pending` for recovery to replay.
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

    fn create(config: &Config, store: Option<Arc<dyn KvStore>>) -> crate::Result<Arc<Runtime>> {
        // let scher = Scheduler::new();
        let env = Arc::new(Enviroment::new());
        let cache = Arc::new(Cache::new(config, store)?);
        let emitter = Arc::new(Emitter::new());
        let package = Arc::new(Package::new());
        let queue = Queue::new();
        let shutdown = CancellationToken::new();
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
                            let exposes = &proc.model().exposes;
                            if !exposes.is_empty() {
                                // validate the process outputs
                                let schema = crate::ActSchema::Multiple(exposes.clone());
                                if let Err(e) = schema
                                    .validate(&(message.outputs.to_value()))
                                    .map_err(|err| {
                                        ActError::Model(format!(
                                            "model({}) outputs validation error: {}",
                                            proc.model().id,
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
