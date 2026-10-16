//! The runtime itself: the composition root of the engine.
//!
//! [`Runtime`] wires the store (and its write-behind queue), the emitter, the
//! package registry, the schema cache, the snapshot registry and the shell —
//! the operation driver — together, and hosts the store-facing timers (schedule
//! triggers, delivery retries, snapshot TTL). It is the entry point every
//! serving path holds; the names a consumer outside the runtime may use are
//! re-exported from `mod.rs`.

use crate::{
    ActError, Action, Config, Package, Result, ShareLock, Vars, Workflow, data,
    env::Environment,
    event::{Emitter, ProcessGate},
    scheduler::shell::{
        Process,
        health::{LoopGuard, SchedulerHealth},
        validation::SchemaCache,
    },
    snapshot::{SnapshotOptions, SnapshotStore},
    store::{KvStore, Store, writebehind},
    utils::{self, LaneRouter, consts},
};

use parking_lot::RwLock;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{runtime::Handle, time};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, instrument};

#[derive(Clone)]
pub struct Runtime {
    config: Arc<Config>,
    env: Arc<Environment>,
    /// The durable store every read API and the kernel driver go through.
    store: Arc<Store>,
    /// The read model's write-behind queue: delivery rows and process
    /// removals are ordered per pid and must not run inline while an operation
    /// is in flight.
    writer: writebehind::StoreWriter,
    /// The pid → lane mapping shared with the emitter's handler gate. Held
    /// here so a sweeper removal can release the pid's assignment.
    router: Arc<LaneRouter>,
    emitter: Arc<Emitter>,
    package: Arc<Package>,
    shutdown: CancellationToken,
    schema_cache: Arc<SchemaCache>,
    pub(crate) snapshots: Arc<SnapshotRegistry>,
    /// Failure state of the schedule-trigger timer (see [`SchedulerHealth`]).
    trigger_health: Arc<LoopGuard>,
    /// Failure state of the message-retry timer (see [`SchedulerHealth`]).
    retry_health: Arc<LoopGuard>,
    /// The deterministic kernel host: it *is* the engine — it owns every
    /// started process (start, actions, effects, messages) and drives the operations
    /// over the aggregate documents.
    kernel: Arc<crate::scheduler::shell::KernelHost>,
}

/// Tick the periodic timers run on under test — short enough that a test can
/// watch several ticks, and the one source of truth for tests that reason in
/// ticks.
#[cfg(test)]
pub(crate) const TEST_TICK_MS: u64 = 800;

/// How many terminal-but-unmarked runs one sweep re-projects before selecting
/// (see [`Runtime::sweep_removable`]): the same order as the sweep's own page,
/// so one pass over the collection can repair and then reclaim.
const SWEEP_RECONCILE_LIMIT: usize = 256;

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
            .field("env", &self.env)
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
    ) -> crate::Result<Arc<crate::scheduler::shell::validation::CachedPackage>> {
        let store = self.store();
        self.schema_cache.package(&store, uses).await
    }

    pub fn store(&self) -> Arc<Store> {
        self.store.clone()
    }

    pub fn store_writer_depth(&self) -> usize {
        self.writer.depth()
    }

    pub fn store_writer_high_watermark(&self) -> usize {
        self.writer.high_watermark()
    }

    /// Whether the store write path is saturated and therefore refusing new
    /// work (a task's state write, or the outbox record standing in for it)
    /// until its backlog drains.
    pub fn store_writer_saturated(&self) -> bool {
        self.writer.saturated()
    }

    /// Store one message's canonical row and one delivery row through the
    /// write-behind queue (see [`writebehind::store_delivery`]).
    pub(crate) async fn store_delivery(
        &self,
        message: &crate::data::Message,
        delivery: &crate::data::Delivery,
    ) -> Result<bool> {
        writebehind::store_delivery(&self.writer, message, delivery).await
    }

    /// The sweeper: delete every finished process whose deliveries settled
    /// (see [`writebehind::sweep_removable`]).
    ///
    /// A settled run's `removable` mark is re-derived here first. The mark is
    /// written by the projection of the run's own commits, and a settled run
    /// has none left: its last commit can predate the delivery that settled
    /// its outbox records (the delivery pass refreshes the row only for the
    /// runs it delivered for, and nothing repairs a settled run's row), so a
    /// terminal row that still reads not-removable would otherwise keep the
    /// run's rows — and the directory swept with them — forever.
    pub(crate) async fn sweep_removable(&self) -> Result<usize> {
        for pid in self
            .store
            .unmarked_settled_procs(SWEEP_RECONCILE_LIMIT)
            .await?
        {
            self.kernel.refresh_projection(&pid).await;
        }
        writebehind::sweep_removable(&self.store, &self.writer, &self.router).await
    }

    /// Global write-behind barrier — test visibility (a test that asserted on
    /// projected rows must see every queued write applied first).
    #[cfg(test)]
    pub(crate) async fn flush_writer(&self) -> Result<()> {
        self.writer.flush().await
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
        self.writer.close().await;
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
        // Restart discrimination. The pid must be free: an existing pid is
        // refused. The durable record is the aggregate document (the proc/task
        // rows are its projection of the moment, and a settled run's rows are
        // swept while its aggregate stays); a leftover proc row is refused
        // too, so a run whose aggregate is gone cannot be restarted under a
        // pid the row still names. The check runs before anything durable: a
        // refusal consumes nothing.
        let occupied = self.kernel.exists(&proc_id).await?
            || self.store.procs().find_opt(&proc_id).await?.is_some();
        if occupied {
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
        // The process directory root comes from the engine config, never from
        // the request: a caller can neither name the directory nor place a
        // run outside it. The directory lives exactly as long as the
        // process's durable rows — the sweeper removes it with them, and a
        // start that never became durable removes it itself (see
        // `Cache::abandon`).
        if let Some(root) = self.config.workdir() {
            let dir = prepare_workdir(&root, &proc_id)?;
            proc.set_workdir(&dir);
        }
        if let Some(owner) = &owner {
            proc.set_owner_scope(owner);
        }

        // The kernel owns the run: the aggregate is its record from here on.
        // The `Process` built above is only the handle this call returns for
        // the caller's own use (its id); nothing keeps it resident, and the
        // durable read model arrives with the row projection. The model facts,
        // the run's authority and its start inputs all seed the aggregate.
        let package = self.package.clone();
        let run_as = move |id: &str| package.get(id).map(|register| (register.meta)().run_as);
        // An unimportable model is refused here, not silently skipped.
        let spec = crate::scheduler::shell::ProcessSpec::from_workflow(&proc_id, model, &run_as)?
            .with_owner(owner.clone().unwrap_or_default())
            .with_vars(&options);
        self.kernel.start_process(&spec).await?;
        info!(pid = %proc_id, mid = %model.id, name = %model.name, "process started");
        Ok(proc)
    }

    /// The kernel host — the engine. The driver tests read it to inspect an
    /// aggregate; the serving paths hold the field directly.
    #[cfg(test)]
    pub(crate) fn kernel(&self) -> &Arc<crate::scheduler::shell::KernelHost> {
        &self.kernel
    }

    /// The read model for one process. There is no resident object — a
    /// process's own record is the aggregate document and the rows are its
    /// projection, read straight from the store.
    pub async fn proc(self: &Arc<Self>, pid: &str) -> Result<Option<Arc<Process>>> {
        self.store().load_proc(pid, self).await
    }

    #[instrument(skip(self, action), fields(pid = %action.pid, tid = %action.tid, event = ?action.event))]
    pub async fn do_action(self: &Arc<Self>, action: &Action) -> Result<()> {
        // The kernel serves the action: the aggregate is the record and
        // `apply_action` re-plans against its authoritative snapshot.
        Self::kernel_action(&self.kernel, action).await
    }

    /// Apply one action to `tid` — the shape a test's message handler calls
    /// (the wire `Action` is what production sends).
    #[cfg(test)]
    pub async fn do_action2(
        self: &Arc<Self>,
        pid: &str,
        tid: &str,
        action: crate::event::EventAction,
        options: crate::Vars,
    ) -> Result<()> {
        self.do_action(&Action::new(pid, tid, action, options))
            .await
    }

    /// Serve one client action through the kernel host. The kernel's messages
    /// are keyed by model node id, so the client's `tid` is already the
    /// execution's — no act-up-to-step resolution is involved. The action is
    /// planned against the aggregate's authoritative snapshot: an action that
    /// lands while the target is still propagating is refused with
    /// `ActionNotAllowed` and the client retries, exactly as the legacy
    /// resident check refused a non-resident process.
    async fn kernel_action(
        host: &Arc<crate::scheduler::shell::KernelHost>,
        action: &Action,
    ) -> Result<()> {
        let Some(kind) = Self::kernel_kind(action)? else {
            return Err(ActError::Action(format!(
                "action {:?} is not served by the kernel engine",
                action.event
            )));
        };
        let inputs: Vec<(String, String)> = action
            .options
            .iter()
            .map(|(key, value)| match value {
                serde_json::Value::String(text) => (key.clone(), text.clone()),
                other => (key.clone(), other.to_string()),
            })
            .collect();
        host.apply_action(&action.pid, &action.tid, kind, inputs)
            .await
    }

    /// Map a wire event to its kernel action. `Next` is the wire name of
    /// "complete"; `Push`/`Remove`/`SetProcessVars` have no kernel equivalent
    /// yet.
    ///
    /// The error action carries the *legacy* input names — `ecode` (required
    /// there, and what `$ecode()` reads from the task's data) and `message` —
    /// so a client's error means the same thing as it did on the legacy
    /// engine.
    fn kernel_kind(
        action: &Action,
    ) -> Result<Option<crate::scheduler::core::commit::command::ActionKind>> {
        use crate::event::EventAction as E;
        use crate::scheduler::core::commit::command::ActionKind as K;
        use crate::utils::consts as names;
        let kind = match action.event {
            E::Next => K::Complete,
            E::Submit => K::Submit,
            E::Back => K::Back,
            E::Cancel => K::Cancel,
            E::Abort => K::Abort,
            E::Skip => K::Skip,
            E::Error => {
                let Some(code) = action.options.get::<String>(names::ACT_ERR_CODE) else {
                    return Err(ActError::Action(format!(
                        "cannot find '{}' in options",
                        names::ACT_ERR_CODE
                    )));
                };
                K::Fail {
                    code,
                    message: action
                        .options
                        .get::<String>(names::ACT_ERR_MESSAGE)
                        .unwrap_or_default(),
                }
            }
            E::Push | E::Remove | E::SetProcessVars => return Ok(None),
        };
        Ok(Some(kind))
    }

    /// Ack one delivery row (by its delivery id).
    pub async fn ack(&self, id: &str) -> Result<()> {
        self.store
            .set_delivery(id, data::DeliveryStatus::Acked)
            .await
    }

    fn create(config: &Config, store: Option<Arc<dyn KvStore>>) -> crate::Result<Arc<Runtime>> {
        // refuse a config that switches the runaway protections off
        config.validate()?;
        let env = Arc::new(Environment::new());
        // The durable store, and its write-behind queue: one pid → lane
        // mapping for the emitter's handler gate (a removal releases its
        // assignment), and the writer's own shard router.
        let router = Arc::new(LaneRouter::new(config.scheduler_workers()));
        let store = Arc::new(Store::new(
            store.unwrap_or_else(|| Arc::new(crate::store::MemoryStore::new())),
        ));
        let writer = writebehind::StoreWriter::spawn(
            store.clone(),
            config.store_writer_workers(),
            config.store_writer_queue_cap(),
            utils::shared(config.store_writer_workers()),
        );
        let process_gate = ProcessGate::with_router(router.clone());
        let emitter = Arc::new(Emitter::with_process_gate(process_gate));
        let package = Arc::new(Package::new());
        let shutdown = CancellationToken::new();
        let schema_cache = Arc::new(SchemaCache::new());
        let snapshots = Arc::new(SnapshotRegistry::new());
        // The deterministic kernel host — the engine. It owns its own driver
        // task over the same key-value store, woken per operation, ticked for
        // recovery and outbox delivery, and it serves (starts, actions,
        // effects, messages).
        let (kernel, wake_rx) = crate::scheduler::shell::KernelHost::new(
            store.kv(),
            config.cache_cap() as usize,
            config.max_node_run_times() as u64,
        );
        {
            let driver = kernel.clone();
            let driver_shutdown = shutdown.clone();
            // One rule for the driver's cadence and the periodic timers (see
            // [`Self::tick_interval`]): the outbox delivery pass and the
            // recovery scan run on it, so a test that waits for a message does
            // not have to wait the production 15 s.
            let tick = Self::tick_interval(config);
            tokio::spawn(async move {
                driver.serve(wake_rx, driver_shutdown, tick).await;
            });
        }
        let runtime = Arc::new(Runtime {
            config: Arc::new(config.clone()),
            emitter,
            env,
            store,
            writer,
            router,
            package,
            shutdown,
            schema_cache,
            snapshots,
            trigger_health: LoopGuard::new("schedule-trigger"),
            retry_health: LoopGuard::new("message-retry"),
            kernel,
        });

        // The driver seams: the host serves, so the effect runner (Func
        // packages execute for real) and the outbox transport (records become
        // engine messages) are attached to the serving host. The
        // process-lifecycle sink mirrors the legacy `on_proc` events.
        let host = &runtime.kernel;
        host.attach_effect_runner(Arc::new(crate::scheduler::shell::PackageEffectRunner::new(
            runtime.clone(),
        )));
        let emitter = runtime.emitter.clone();
        host.attach_outbox_transport(Box::new(crate::scheduler::shell::MessageForwarder::new(
            move |msg: &crate::Message| emitter.emit_message(msg),
        )));
        let emitter = runtime.emitter.clone();
        host.attach_lifecycle_sink(Box::new(move |msg, event| match event {
            crate::scheduler::shell::ProcessEvent::Started => emitter.emit_start_event(msg),
            crate::scheduler::shell::ProcessEvent::Completed => emitter.emit_complete_event(msg),
            crate::scheduler::shell::ProcessEvent::Errored => emitter.emit_error(msg),
        }));
        // The read model: every commit re-materializes the legacy
        // proc/task/vars rows from the aggregate, so the row-reading APIs
        // (`proc:get`/`task:get`/`msg:ls`, `Runtime::proc`) and the sweeper
        // (which reclaims a settled run's rows and directory) see the
        // kernel's runs exactly like the legacy engine's.
        host.attach_row_projector(Arc::new(crate::scheduler::shell::RowProjector::new(
            runtime.store(),
            runtime.writer.clone(),
            runtime.config().workdir(),
        )));
        // Branch predicates are decided on the engine's own environment
        // (the legacy `ctx.eval`: engine functions included) on a headless
        // task of the declaring model.
        host.attach_predicate_runner(Arc::new(
            crate::scheduler::shell::EnginePredicateRunner::new(runtime.clone()),
        ));
        Ok(runtime)
    }

    /// The periodic timers' interval: the configured tick, or the engine's
    /// default of 15s, and [`TEST_TICK_MS`] under test. One rule for every
    /// timer that reasons in ticks.
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

    /// Message-retry timer — periodically re-sends unacknowledged messages and
    /// sweeps the settled processes whose deliveries have all landed.
    ///
    /// Both jobs are the kernel's: the delivery rows are written through the
    /// channel path (which the kernel's outbox transport feeds) and the rows
    /// themselves are the kernel's projection, so the two store-facing loops
    /// run here rather than inside the driver — they are not part of the operation
    /// protocol and must not delay it.
    pub fn init_retry_timer(self: &Arc<Self>) -> crate::Result<()> {
        let max_message_retry_times = self.config().max_message_retry_times();
        let interval_ms = Self::tick_interval(self.config()).as_millis() as u64;

        let evt = self.emitter().clone();
        let store = self.store.clone();
        let shutdown = self.shutdown.clone();
        let health = self.retry_health.clone();
        let sweeper = self.clone();
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
                match store
                    .with_no_response_deliveries(interval_ms as i64, max_message_retry_times)
                    .await
                {
                    Ok(rearmed) => {
                        for d in rearmed {
                            let store = store.clone();
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
                if let Err(err) = sweeper.sweep_removable().await {
                    error!(error = %err, "settled-process sweep failed");
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
        let model = self.store.models().find(&event.mid).await?;
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
            Some(schedule) => crate::scheduler::core::plan::cron::Cron::next_fire_millis(schedule),
            None => 0,
        };
        self.store.events().update(&event).await?;
        started.map(|_| ())
    }
}

/// The runtime as a package hook sees it: the store it reads through, and a
/// start that reports its process id (see [`crate::package::ActPackageRuntime`]).
///
/// Implemented on `Arc<Runtime>`, not `Runtime`: [`Runtime::start`] needs a
/// shared handle, and this is the shape the hook's `&dyn ActPackageRuntime`
/// parameter coerces to at the call site.
#[async_trait::async_trait]
impl crate::package::ActPackageRuntime for Arc<Runtime> {
    fn store(&self) -> Arc<Store> {
        Runtime::store(self)
    }

    async fn start(&self, model: &Workflow, options: Vars) -> Result<String> {
        let proc = Runtime::start(self, model, options).await?;
        Ok(proc.id().to_string())
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
pub(crate) fn prepare_workdir(root: &std::path::Path, pid: &str) -> Result<std::path::PathBuf> {
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
