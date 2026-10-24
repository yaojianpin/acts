//! The runtime itself: the composition root of the engine.
//!
//! [`Runtime`] wires the store (and its write-behind queue), the emitter, the
//! package registry, the schema cache, the snapshot registry and the shell —
//! the operation driver — together, and hosts the store-facing timers (schedule
//! triggers, delivery retries, snapshot TTL). It is the entry point every
//! serving path holds; the names a consumer outside the runtime may use are
//! re-exported from `mod.rs`.

use crate::{
    ActError, Action, Config, Package, Result, Vars, Workflow, data,
    env::Environment,
    event::{Emitter, ProcessGate},
    scheduler::{
        core::health::{CleanupMetrics, SchedulerHealth},
        shell::{
            KernelHost, ProcRead, SnapshotRegistry, TimerGuider, timers as shell_timers,
            validation::SchemaCache,
        },
    },
    snapshot::{SnapshotOptions, SnapshotStore},
    store::{KvStore, Store, writebehind},
    utils::{self, LaneRouter, consts},
};

use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, warn};

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
    /// The deterministic kernel host: it *is* the engine — it owns every
    /// started process (start, actions, effects, messages) and drives the operations
    /// over the aggregate documents, including the store-facing timers
    /// (`timer_guider`/`timer_guards`).
    kernel: Arc<KernelHost>,
}

/// Tick the periodic timers run on under test — short enough that a test can
/// watch several ticks, and the one source of truth for tests that reason in
/// ticks.
#[cfg(test)]
pub(crate) const TEST_TICK_MS: u64 = 800;

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

    /// The pid → lane mapping, so the timers' cleanup can release a swept
    /// run's assignment.
    pub(crate) fn router(&self) -> &Arc<LaneRouter> {
        &self.router
    }

    /// The write-behind queue, for the timers' barriered removals.
    pub(crate) fn writer(&self) -> &writebehind::StoreWriter {
        &self.writer
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

    /// Global write-behind barrier — test visibility (a test that asserted on
    /// projected rows must see every queued write applied first).
    #[cfg(test)]
    pub(crate) async fn flush_writer(&self) -> Result<()> {
        self.writer.flush().await
    }

    /// Failure state of the store-facing background loops: consecutive failed
    /// ticks and the backoff window each is on (see [`SchedulerHealth`]). A
    /// loop whose store keeps failing is reported `Degraded` and skips ticks
    /// instead of polling the store at the full tick rate, so this is where a
    /// host reads *why* scheduled triggers are late or deliveries are not
    /// being re-sent. The kernel host owns every loop's guard — the four
    /// driver passes and the four timers — so it answers the whole state.
    pub fn scheduler_health(&self) -> SchedulerHealth {
        self.kernel.scheduler_health()
    }

    /// The ready/timer pass's backlog and age gauges (A10).
    pub fn ready_metrics(&self) -> crate::scheduler::shell::ReadyMetrics {
        self.kernel.ready_metrics()
    }

    /// The outbox retry pass's backlog and age gauges (A10).
    pub fn outbox_metrics(&self) -> crate::scheduler::shell::OutboxMetrics {
        self.kernel.outbox_metrics()
    }

    /// The recovery scanner's backlog and counters (A10).
    pub fn recovery_metrics(&self) -> crate::scheduler::shell::RecoveryMetrics {
        self.kernel.recovery_metrics()
    }

    /// The terminal-run cleanup's counters (see [`CleanupMetrics`]).
    pub fn cleanup_metrics(&self) -> CleanupMetrics {
        self.kernel.cleanup_metrics()
    }

    /// The read model's counters and repair backlog (A5): rows applied,
    /// writes refused as stale, projection failures, repaired runs, and the
    /// gauge of runs whose rows are currently behind their aggregate.
    pub fn projection_metrics(&self) -> crate::scheduler::shell::ProjectionMetrics {
        self.kernel.projection_metrics()
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
    pub async fn start(self: &Arc<Self>, model: &Workflow, mut options: Vars) -> Result<String> {
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
        // Restart discrimination. The aggregate is the pid's durable existence
        // record: a pid with an aggregate is taken, and only after the
        // authoritative batch removed it can the same pid start again. The
        // per-pid lifecycle lock makes that decision atomic with the cleanup:
        // a cleanup that already deleted the aggregate keeps the lock (and its
        // `reclaiming` mark) until the derived rows and workdir are gone, so a
        // new run cannot be created in the middle of the old run's residue
        // removal. No row family — and in particular no legacy `ops` row,
        // which L1 removes — participates in pid existence.
        let _lifecycle = self.kernel.lock_lifecycle(&proc_id).await;
        let occupied = self.kernel.exists(&proc_id).await? || self.kernel.is_reclaiming(&proc_id);
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

        // The run's declaration is validated before anything durable exists:
        // the legacy start validated it while loading its process, and the
        // kernel import must refuse the same malformed models (ids, `next`
        // targets, triggers, `rn`) rather than seed an aggregate.
        model.valid()?;

        // The process directory root comes from the engine config, never from
        // the request: a caller can neither name the directory nor place a
        // run outside it. The directory lives exactly as long as the
        // process's durable rows — the sweeper removes it with them, and a
        // start that never became durable removes it itself (see
        // `Cache::abandon`).
        if let Some(root) = self.config.workdir() {
            prepare_workdir(&root, &proc_id)?;
        }

        // The kernel owns the run: the aggregate is its record from here on.
        // The model facts, the run's authority and its start inputs all seed
        // the aggregate; nothing is kept resident, and the durable read model
        // arrives with the row projection.
        let package = self.package.clone();
        let run_as = move |id: &str| package.get(id).map(|register| (register.meta)().run_as);
        // An unimportable model is refused here, not silently skipped.
        let spec = crate::scheduler::shell::ProcessSpec::from_workflow(&proc_id, model, &run_as)?
            .with_owner(owner.clone().unwrap_or_default())
            .with_vars(&options);
        self.kernel.start_process(&spec).await?;
        info!(pid = %proc_id, mid = %model.id, name = %model.name, "process started");
        Ok(proc_id)
    }

    /// The kernel host — the engine. The driver tests read it to inspect an
    /// aggregate; the serving paths hold the field directly, and the public
    /// reconciliation API (`Engine::effect_unknowns`/`resolve_effect`) needs
    /// the same aggregate view.
    pub(crate) fn kernel(&self) -> &Arc<crate::scheduler::shell::KernelHost> {
        &self.kernel
    }

    /// The read model for one process. There is no resident object — a
    /// process's own record is the aggregate document and the rows are its
    /// projection, read straight from the store.
    ///
    /// The rows are projected from the aggregate, and a read is a *strong*
    /// read with respect to that projection: if the proc row's
    /// `source_revision` is behind the durable aggregate (a failed projection,
    /// a crash between commit and projection, an older database), the
    /// aggregate is re-projected synchronously before the rows are loaded.
    /// The aggregate is the authority; the rows never decide anything on
    /// their own. A repair that fails is queued and reported, and the read
    /// still returns the best row the store holds — callers can observe the
    /// gap through [`Self::projection_metrics`].
    pub(crate) async fn read_proc(self: &Arc<Self>, pid: &str) -> Result<Option<ProcRead>> {
        self.refresh_read_model(pid).await;
        crate::scheduler::shell::load_proc_rows(&self.store(), pid).await
    }

    /// The run's declared model graph (`proc:graph`): every node the run's
    /// durable (materialized) model holds, executed or not, in declaration
    /// order. The ids are the ones the run's messages carry as `nid`, so a
    /// client renders the full flow and folds the message stream onto it; the
    /// import's virtual container is kind `block` (the declaration declares
    /// no such node), which the tree display folds into its parent step.
    pub(crate) async fn graph(&self, pid: &str) -> Result<crate::ProcessGraph> {
        self.kernel.graph(pid).await
    }

    /// The on-demand half of the projection protocol (see [`Self::read_proc`]).
    async fn refresh_read_model(&self, pid: &str) {
        let Some(projector) = self.kernel.projector() else {
            return;
        };
        let snapshot = match self.kernel.snapshot(pid).await {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => return,
            Err(error) => {
                warn!(pid, error = %error, "read model refresh could not load the aggregate");
                return;
            }
        };
        let row_version = match self.store().procs().find_opt(pid).await {
            Ok(Some(row)) => {
                crate::scheduler::core::model::transition_plan::ProcessVersion::decode(
                    &row.source_revision,
                )
            }
            Ok(None) => None,
            Err(error) => {
                warn!(pid, error = %error, "read model refresh could not read the proc row");
                return;
            }
        };
        if row_version.is_some_and(|version| version >= snapshot.version) {
            return;
        }
        match projector.project_repair(&snapshot).await {
            Ok(_) => projector.finish_repair(pid),
            Err(error) => {
                projector.enqueue_repair(pid);
                warn!(
                    pid,
                    error = %error,
                    "read model refresh failed; serving the rows already stored and queued a repair"
                );
            }
        }
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
        let kind = Self::kernel_kind(action)?;
        let inputs = Self::kernel_inputs(&action.options);
        host.apply_action(&action.pid, &action.tid, kind, inputs)
            .await
    }

    /// Map a wire event to its kernel action. `Next` is the wire name of
    /// "complete"; `Push` and `Remove` are the countersign pair — a push creates
    /// a child act under the execution, a remove decides it `Removed`.
    ///
    /// The error action carries the *legacy* input names — `ecode` (required
    /// there, and what `$ecode()` reads from the task's data) and `message` —
    /// so a client's error means the same thing as it did on the legacy
    /// engine. A push's `uses` is the same kind of caller-correctable input:
    /// the legacy `dispatch_act` refused its absence before anything durable
    /// existed.
    pub(crate) fn kernel_kind(
        action: &Action,
    ) -> Result<crate::scheduler::core::commit::command::ActionKind> {
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
            E::Push => {
                let uses = action.options.get::<String>("uses");
                if uses.as_deref().unwrap_or_default().trim().is_empty() {
                    return Err(ActError::Action("cannot find 'uses' in act".to_string()));
                }
                K::Push
            }
            E::Remove => K::Remove,
        };
        Ok(kind)
    }

    /// The wire options as the kernel's decision inputs: text stays text,
    /// everything else rides its JSON rendering.
    pub(crate) fn kernel_inputs(options: &Vars) -> Vec<(String, String)> {
        options
            .iter()
            .map(|(key, value)| match value {
                serde_json::Value::String(text) => (key.clone(), text.clone()),
                other => (key.clone(), other.to_string()),
            })
            .collect()
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
        // effects, messages). Its serve loop also ticks the runtime's
        // store-facing timers once the guider is attached below, so ONE
        // background task owns every store-reaching pass.
        let schedule = shell_timers::kernel_schedule(config);
        let (kernel, wake_rx) = KernelHost::new_with_caps(
            store.kv(),
            KernelHost::caps_from_config(config),
            config.max_node_run_times() as u64,
            // A timer's idle round is deferred by the ready/timer interval,
            // not by the recovery or outbox cadence (A10).
            schedule.ready.as_millis() as i64,
        );
        {
            let driver = kernel.clone();
            let driver_shutdown = shutdown.clone();
            // Each control-plane loop runs on its own interval (A10); under
            // test every unset interval follows the short test tick, so a test
            // that waits for a message does not have to wait the production 15 s.
            tokio::spawn(async move {
                driver.serve(wake_rx, driver_shutdown, schedule).await;
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
        // (`proc:get`/`task:get`/`msg:ls`, `Runtime::read_proc`) and the sweeper
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
        // The store-facing timers ride the host's serve loop: the guider's
        // four passes are ticks of the ONE background task that reaches the
        // store, not separate spawned loops.
        host.attach_timers(Arc::new(TimerGuider::new(&runtime)));
        Ok(runtime)
    }

    /// One terminal-run cleanup pass: reclaim the durable state of finished
    /// runs, then delete their rows and workdirs. The pass itself lives with
    /// the timers ([`shell_timers`]) and runs on the host's serve loop; this
    /// entry is the test suites' explicit reclamation seam.
    #[cfg(test)]
    pub(crate) async fn sweep_removable(self: &Arc<Self>) -> Result<usize> {
        shell_timers::sweep_removable(self).await
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
        Runtime::start(self, model, options).await
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

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime() -> Arc<Runtime> {
        Runtime::new(&Config::default(), None).unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn accessors_report_the_wired_components() {
        let runtime = runtime();
        let registry = runtime.snapshot_registry();
        assert_eq!(registry.len(), 0);
        registry.register("profile", SnapshotOptions::default());
        assert_eq!(registry.len(), 1);
        assert!(registry.store("profile").is_some());
        assert_eq!(registry.list().len(), 1);
        assert_eq!(registry.purge_expired(), 0);

        let debug = format!("{runtime:?}");
        assert!(debug.contains("Runtime"));
        assert!(
            debug.contains("<1 entries>"),
            "the snapshot count is rendered"
        );
        assert!(debug.contains("health"));

        // The writer gauges and the loop metrics are readable straight away.
        let _ = runtime.store_writer_depth();
        let _ = runtime.store_writer_high_watermark();
        let _ = runtime.store_writer_saturated();
        let _ = runtime.ready_metrics();
        let _ = runtime.outbox_metrics();
        let _ = runtime.recovery_metrics();
        let _ = runtime.projection_metrics();
        let _ = runtime.cleanup_metrics();
        let _ = runtime.scheduler_health();
        let _ = runtime.config().scheduler_workers();

        runtime.close().await;
    }

    #[test]
    fn workdir_names_must_be_safe_segments() {
        let root = std::env::temp_dir().join(format!("acts-wd-{}", crate::utils::longid()));
        for pid in ["", ".", "..", "a/b", "a\\b", "c:", "a\0b"] {
            assert!(
                prepare_workdir(&root, pid).is_err(),
                "{pid:?} is not a single path component"
            );
        }
        let dir = prepare_workdir(&root, "run1").unwrap();
        assert!(dir.is_dir());
        // A pid already occupied by a file cannot be materialized.
        std::fs::write(root.join("occupied"), b"x").unwrap();
        let error = prepare_workdir(&root, "occupied").unwrap_err();
        assert!(error.to_string().contains("failed to create"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn an_error_action_requires_its_code_and_a_push_its_uses() {
        use crate::event::EventAction;

        let runtime = runtime();
        let error = runtime
            .do_action(&Action::new("pid", "tid", EventAction::Error, Vars::new()))
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains(crate::utils::consts::ACT_ERR_CODE)
        );

        // A push without its act's `uses` is the caller-correctable refusal
        // the legacy `dispatch_act` answered with.
        let error = runtime
            .do_action(&Action::new("pid", "tid", EventAction::Push, Vars::new()))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("cannot find 'uses' in act"));
        runtime.close().await;
    }
}

#[cfg(test)]
mod wire_tests {
    use super::*;

    #[test]
    fn an_error_action_carries_its_legacy_code_and_message() {
        use crate::event::EventAction as E;
        use crate::scheduler::core::commit::command::ActionKind as K;

        let options = Vars::new()
            .with(crate::utils::consts::ACT_ERR_CODE, "E1")
            .with(crate::utils::consts::ACT_ERR_MESSAGE, "boom");
        let kind = Runtime::kernel_kind(&Action::new("p", "t", E::Error, options)).unwrap();
        assert_eq!(
            kind,
            K::Fail {
                code: "E1".into(),
                message: "boom".into()
            }
        );

        // A missing message is an empty one, not a failure.
        let kind = Runtime::kernel_kind(&Action::new(
            "p",
            "t",
            E::Error,
            Vars::new().with(crate::utils::consts::ACT_ERR_CODE, "E2"),
        ))
        .unwrap();
        assert_eq!(
            kind,
            K::Fail {
                code: "E2".into(),
                message: String::new()
            }
        );
    }
}

#[cfg(test)]
mod cleanup_tests {
    use super::*;

    /// A reclaim mark left by an interrupted cleanup whose rows disappeared
    /// anyway must be cleared, so the pid is not blocked forever.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn a_reclaim_mark_without_rows_is_cleared() {
        let runtime = Runtime::new(&Config::default(), None).unwrap();
        let pid = format!("ghost-{}", crate::utils::longid());
        runtime.kernel().begin_reclaim(&pid);
        assert!(runtime.kernel().is_reclaiming(&pid));
        assert_eq!(runtime.sweep_removable().await.unwrap(), 0);
        assert!(
            !runtime.kernel().is_reclaiming(&pid),
            "the mark is cleared once the rows are provably gone"
        );
        runtime.close().await;
    }
}

#[cfg(test)]
mod scope_tests {
    use super::*;

    /// `resolve_scope_params` is exercised from here because `TaskId` is a
    /// core (scheduler-private) type: inside the scheduler this test can name
    /// it, while `snapshot.rs` (crate root) cannot.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn scope_params_resolve_in_order_and_report_missing_names() {
        use crate::scheduler::core::model::task_graph::TaskId;

        let runtime = Runtime::new(&Config::default(), None).unwrap();
        let model = crate::model::Workflow::new()
            .with_id("wf-scope")
            .with_step(|step| step.with_id("s1"));
        let spec = crate::scheduler::ScopeSpec {
            process_id: "p-scope".into(),
            node: "s1".into(),
            task_id: TaskId::new("s1", 0).unwrap(),
            model,
            pushed: None,
            vars: vec![("known".into(), serde_json::json!("value").to_string())],
            env: Vec::new(),
            owner: crate::ScopePolicy::deny_all(),
        };
        let (scope, task, _ctx) = crate::scheduler::headless_scope(&runtime, &spec).unwrap();

        let options = SnapshotOptions {
            scope: vec!["known".into()],
            ..SnapshotOptions::default()
        };
        assert_eq!(
            crate::snapshot::resolve_scope_params(&task, &scope, &options).unwrap(),
            vec![serde_json::json!("value")]
        );

        let options = SnapshotOptions {
            scope: vec!["known".into(), "missing".into(), "also-missing".into()],
            ..SnapshotOptions::default()
        };
        assert_eq!(
            crate::snapshot::resolve_scope_params(&task, &scope, &options).unwrap_err(),
            vec!["missing".to_string(), "also-missing".to_string()]
        );
        assert_eq!(crate::snapshot::join_scope(&[]), "");

        runtime.close().await;
    }

    /// A pid left marked by a failed cleanup whose rows are gone anyway is
    /// unmarked by the sweep, so it is not blocked forever.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn the_sweep_finishes_a_reclaiming_pid_without_rows() {
        let runtime = Runtime::new(&Config::default(), None).unwrap();
        runtime.kernel().begin_reclaim("never-existed");
        assert!(runtime.kernel().is_reclaiming("never-existed"));

        assert_eq!(runtime.sweep_removable().await.unwrap(), 0);
        assert!(
            !runtime.kernel().is_reclaiming("never-existed"),
            "the sweep must clear a mark whose rows are gone"
        );

        runtime.close().await;
    }
}
