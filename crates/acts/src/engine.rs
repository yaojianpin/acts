use crate::{
    AccessControl, ActFunction, ActPackage, ActPlugin, ActUserVar, ChannelOptions, Config,
    Principal, Signal, Vars,
    builder::EngineBuilder,
    export::{Channel, Executor},
    package::{self, ActPackageRegister},
    scheduler::Runtime,
    snapshot::SnapshotManager,
};
use std::{future::Future, pin::Pin, sync::Arc};
use tokio_util::sync::CancellationToken;

/// One operation whose external effect may have run but whose commit was lost
/// (A1c).
///
/// Such an operation is deliberately not re-driven: the key is the stable
/// identity the effect executor saw, and the operator either resolves it or
/// lets an executor observation decide it (recovery asks the attached effect
/// runner automatically).
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct EffectUnknown {
    /// The run that owns the operation.
    pub pid: String,
    /// The acting execution's external id (`tid`).
    pub tid: String,
    /// The operation's sequence within that execution.
    pub sequence: u64,
    /// The stable effect key: the token the executor saw and a caller may use
    /// to query the external system.
    pub key: String,
    /// The attempt whose effect became unknown.
    pub attempt: u32,
}

/// One run's outbox records a removed shadow transport confirmed without
/// sending them (see [`Engine::shadow_deliveries`]).
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ShadowDeliveryAudit {
    /// The run that owns the records.
    pub pid: String,
    /// The outbox record keys (the durable message identities).
    pub keys: Vec<String>,
}

/// An operator's durable decision about an unknown effect (A1c).
#[derive(Debug, Clone)]
pub enum EffectResolution {
    /// The external effect ran. `outputs` are the returned values the
    /// operator captured (empty when only the act must settle); they are
    /// folded into the same commit that completes the operation.
    Applied { outputs: Vars },
    /// The effect provably did not run. The operation returns to the retry
    /// queue **under the same effect key**; only do this when the external
    /// step is safe to attempt again (the executor deduplicates the key) or
    /// was never attempted.
    NotApplied,
}

/// A started workflow engine.
///
/// An `Engine` is created only by [`EngineBuilder::start`]. Its runtime is
/// therefore always initialized.
#[derive(Clone)]
pub struct Engine {
    config: Arc<Config>,
    runtime: Arc<Runtime>,
    acl: Arc<dyn AccessControl>,
    /// The plugins this engine started, in prepare order. `close` shuts them
    /// down in reverse, and a failed start's rollback reports how many had
    /// started.
    plugins: Arc<parking_lot::Mutex<Vec<Arc<dyn ActPlugin>>>>,
    started_plugins: Arc<std::sync::atomic::AtomicUsize>,
    /// Whether the engine reached Ready (every registration and plugin start
    /// succeeded, and the start barrier opened). A close before that is a
    /// rollback.
    ready: Arc<std::sync::atomic::AtomicBool>,
    /// `Engine::close` is idempotent: a second call is a no-op.
    closed: Arc<std::sync::atomic::AtomicBool>,
}

impl Engine {
    pub fn builder() -> EngineBuilder {
        EngineBuilder::new()
    }

    pub fn config(&self) -> Arc<Config> {
        self.config.clone()
    }

    /// The engine's access control. A bare engine runs [`AnonymousAcl`]: no
    /// users, no sessions, and no grant for any caller — install
    /// `acts-acl`'s `UserAcl` (see [`EngineBuilder::set_acl`]) for users,
    /// login and sessions.
    pub fn acl(&self) -> Arc<dyn AccessControl> {
        self.acl.clone()
    }

    /// The principal a caller that presents no token resolves to — the
    /// identity to hand [`Engine::executor`] on behalf of such a request.
    ///
    /// It is the anonymous principal: no grant at all, so a login is the way
    /// in (a disabled ACL resolves it to [`Principal::unrestricted`]
    /// instead).
    pub fn anonymous(&self) -> Principal {
        self.acl.anonymous()
    }

    /// Register a package instance from [`ActPlugin::prepare`].
    ///
    /// This writes the package into the runtime registry directly. The start
    /// pass publishes every definition registered this way before
    /// [`EngineBuilder::start`] returns; call it from `prepare` (which runs
    /// before that pass). A call after the start pass installs the runtime
    /// instance but nothing publishes its definition. An embedding application
    /// with no plugin registers packages with [`EngineBuilder::add_package`]
    /// instead.
    pub fn add_package<T>(&self)
    where
        T: ActPackage + 'static,
    {
        let id = T::definition().id;
        self.runtime
            .package()
            .register(id, &ActPackageRegister::new::<T>());
    }

    /// Register a user var module into the engine's expression environment
    /// under `provider` (the registrant's identity: the same var name from
    /// two providers coexists — the first answers — while one provider
    /// registering a name twice is refused).
    ///
    /// Call it from [`ActPlugin::prepare`] (or an embedder that owns the
    /// engine). A transport caller registers through the ACL-checked
    /// `executor.ext().register_var` instead.
    pub fn register_var<T: ActUserVar + Clone + 'static>(
        &self,
        var: &T,
        provider: Option<&str>,
    ) -> crate::Result<()> {
        self.runtime.env().register_var(var, provider)
    }

    /// Register an expression method into the engine's expression environment
    /// under `provider` (see [`Self::register_var`] for the conflict
    /// rules).
    ///
    /// The method is callable by name from a workflow expression
    /// (`owner_of(...)`, or `$owner_of(...)` by the built-in convention). Call
    /// it from [`ActPlugin::prepare`] or an embedder; the ACL-checked runtime
    /// entry is `executor.ext().register_fn`.
    pub fn register_fn<T: ActFunction + Clone + 'static>(
        &self,
        function: &T,
        provider: Option<&str>,
    ) -> crate::Result<()> {
        self.runtime.env().register_fn(function, provider)
    }

    /// Register a snapshot variable at the engine level — the in-process twin
    /// of a provider package's `pack:publish` metadata (`options.var`).
    /// `provider` is the publishing package id (`None` — global); the
    /// `(name, provider)` pair is the conflict key: the same provider twice
    /// is refused, another provider joins the variable's provider list and
    /// must declare the same pull parameters. A run whose sealed context
    /// names the provider is served by it; the rest fall back to the global
    /// registration.
    pub fn register_snap(
        &self,
        name: &str,
        options: crate::SnapshotOptions,
        provider: Option<&str>,
    ) -> crate::Result<()> {
        self.runtime.register_snap(name, options, provider)?;
        Ok(())
    }

    /// The engine's operations, bound to `principal`.
    ///
    /// Every operation of the returned [`Executor`] is checked against that
    /// principal's `allow`/`deny` patterns before it runs — `model().deploy()`
    /// and `proc().start()` included — so an embedder reaches the engine
    /// through exactly the policy a transport caller does. What the principal
    /// may start is what its runs may read: `proc().start()` seals the
    /// principal's [`crate::ScopePolicy`] (snapshot scopes and workdir root)
    /// into the process it starts.
    ///
    /// Use [`Engine::anonymous`] for a request that carries no token, or
    /// [`Principal::unrestricted`] for work that is the engine's own — the
    /// package registrations [`EngineBuilder::start`] performs, a test that
    /// drives the engine directly.
    pub fn executor(&self, principal: &Principal) -> Arc<Executor> {
        Arc::new(Executor::new(&self.runtime, principal))
    }

    /// Event channel (defaults to no redelivery support).
    pub fn channel(&self) -> Arc<Channel> {
        Arc::new(Channel::new(&self.runtime))
    }

    /// Create named channel to receive messages. If `ChannelOptions.id` is
    /// set, unacked messages can be re-sent.
    pub fn channel_with_options(&self, matcher: &ChannelOptions) -> Arc<Channel> {
        Arc::new(Channel::channel(&self.runtime, matcher))
    }

    /// Snapshot manager for feeding snapshot-backed sealed data.
    pub fn snapshot(&self) -> Arc<SnapshotManager> {
        Arc::new(SnapshotManager::new(&self.runtime))
    }

    /// The unknown effects of one run, in operation order (A1c).
    ///
    /// Each entry carries the stable key to correlate with the external
    /// system. Recovery asks the attached effect runner's observation first;
    /// an entry remains here only while nothing could prove applied or
    /// not-applied (or the effect is not idempotent), i.e. while it needs a
    /// human.
    pub async fn effect_unknowns(&self, pid: &str) -> crate::Result<Vec<EffectUnknown>> {
        self.runtime.workers().effect_unknowns(pid).await
    }

    /// Resolve one unknown effect with a durable operator decision (A1c).
    ///
    /// `key` is the [`EffectUnknown::key`] the executor saw. `Applied`
    /// replays the operation's plan without running the effect again;
    /// `NotApplied` returns it to the retry queue under the same key. Both
    /// write through the aggregate's normal guarded commit, so the decision
    /// survives a restart and is never silently redone.
    pub async fn resolve_effect(
        &self,
        pid: &str,
        key: &str,
        resolution: EffectResolution,
    ) -> crate::Result<()> {
        self.runtime
            .workers()
            .resolve_effect_by_key(pid, key, resolution)
            .await
    }

    pub(crate) fn runtime(&self) -> Arc<Runtime> {
        self.runtime.clone()
    }

    /// Operator audit for outbox records a pre-fix build confirmed without
    /// sending (`receipt="shadow"`): the runs whose read model still lists
    /// them, with the record keys. Read-only; a run already reclaimed has no
    /// aggregate to inspect and is not listed.
    ///
    /// The records are not re-sent automatically: the external side may or may
    /// not have received the message, and the engine cannot prove either way.
    pub async fn shadow_deliveries(&self) -> crate::Result<Vec<ShadowDeliveryAudit>> {
        Ok(self
            .runtime
            .shadow_deliveries()
            .await?
            .into_iter()
            .map(|(pid, keys)| ShadowDeliveryAudit { pid, keys })
            .collect())
    }

    /// Close the engine: stop its plugins in reverse start order, then stop
    /// the runtime.
    ///
    /// The shutdown token is cancelled first, so a plugin's tasks see the same
    /// signal the runtime's do; the plugins are then joined, and only after
    /// that does the runtime drain its writer, flush the store and close the
    /// emitter. Closing before Ready is a start rollback: the plugins that
    /// started are still stopped, and the remaining ones answer their default
    /// no-op shutdown.
    pub async fn close(&self) {
        if self.closed.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        tracing::info!(phase = "quiescing");
        self.runtime.shutdown_token().cancel();

        let plugins = self.plugins.lock().clone();
        let started = self
            .started_plugins
            .load(std::sync::atomic::Ordering::SeqCst);
        let mut errors = Vec::new();
        for (index, plugin) in plugins.iter().enumerate().rev() {
            if let Err(error) = plugin.shutdown().await {
                tracing::warn!(
                    plugin = index,
                    error = %error,
                    "plugin shutdown failed"
                );
                errors.push(error.to_string());
            }
        }

        self.runtime.close().await;

        if !self.ready.load(std::sync::atomic::Ordering::SeqCst) {
            // The start never reached Ready: this close is the rollback. The
            // report is a structured log because the builder's Err is the
            // caller's only handle on the failed start.
            tracing::warn!(
                prepared = plugins.len(),
                started,
                stopped = plugins.len(),
                errors = ?errors,
                "startup rollback report"
            );
        }
        tracing::info!(phase = "closed");
    }

    /// Cancellation token fired when the engine shuts down through
    /// [`Engine::close`]. Plugins and embedders that spawn their own
    /// long-running tasks (transport servers, loops) select on it so a
    /// graceful close stops them instead of leaving them to be force-killed
    /// with the process.
    pub fn shutdown_token(&self) -> CancellationToken {
        self.runtime.shutdown_token()
    }

    pub fn signal<T: Clone>(&self, init: T) -> Signal<T> {
        Signal::new(init)
    }

    pub(crate) fn with_runtime(
        config: Arc<Config>,
        runtime: Arc<Runtime>,
        acl: Arc<dyn AccessControl>,
    ) -> crate::Result<Self> {
        // Access control is no longer configured in the file: a stale `[acl]`
        // section is a leftover from the token era and is ignored — loudly,
        // because an operator who still writes one believes it is doing
        // something.
        if config.table.contains_key("acl") {
            tracing::warn!(
                "the config file still has an '[acl]' section: it is no longer read. Users are managed at runtime with acl:setuser / acl:login"
            );
        }

        Ok(Self {
            config,
            runtime,
            acl,
            plugins: Arc::new(parking_lot::Mutex::new(Vec::new())),
            started_plugins: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            ready: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }

    pub(crate) async fn initialize(
        &self,
        plugins: Vec<Arc<dyn ActPlugin>>,
        packages: Vec<ActPackageRegister>,
    ) -> crate::Result<()> {
        // The access control loads before any plugin starts a transport: its
        // users (and the builtin admin, if it has one) exist by the time the
        // first request arrives.
        self.acl.load(self.runtime.store()).await?;
        tracing::info!(phase = "building");

        // A registry that manages users also gates the fetch requests: a
        // `req` reaches only the channels whose subject may read the scope
        // it names (see `AccessControl::principal_of`). Without a user
        // registry there is nothing to ask — channel filters decide.
        if self.acl.manages_users() {
            let acl = self.acl.clone();
            self.runtime.emitter().set_req_gate(Some(Arc::new(
                move |chan_id: String, msg: crate::Message| {
                    let acl = acl.clone();
                    Box::pin(async move {
                        let subject = chan_id.split('/').next().unwrap_or("");
                        let Some(principal) = acl.principal_of(subject).await else {
                            return false;
                        };
                        let target = msg.uses.as_deref().unwrap_or_default();
                        let scope = msg.inputs.get::<String>("scope").unwrap_or_default();
                        match principal.check_scope(target, &scope) {
                            Ok(()) => true,
                            Err(err) => {
                                tracing::debug!(chan = %chan_id, error = %err, "req withheld");
                                false
                            }
                        }
                    }) as Pin<Box<dyn Future<Output = bool> + Send>>
                },
            )));
        }

        *self.plugins.lock() = plugins.clone();
        // Phase 1: validate every plugin and let it register what it provides.
        // Nothing listens yet, so a failure here aborts with no resource to
        // undo.
        for (index, plugin) in plugins.iter().enumerate() {
            plugin.prepare(self).await?;
            tracing::debug!(plugin = index, phase = "plugin-prepared");
        }
        self.register_packages(packages).await?;
        tracing::info!(phase = "prepared", plugins = plugins.len());

        // The start assembly's own precondition: every registered `uses` step
        // must be executable once the barrier opens. The builder's runtime
        // attaches the package runner in `Runtime::create`; a future assembly
        // that forgot it would fail here instead of deferring every act after
        // Ready.
        if !self.runtime.workers().effect_runner_attached() {
            return Err(crate::ActError::Runtime(
                "the start assembly has no effect runner attached".into(),
            ));
        }

        // Phase 2: bind listeners and spawn tasks. A failure here returns to
        // the builder, whose error path closes the engine: plugins started so
        // far are shut down in reverse order and the runtime never reaches
        // Ready.
        tracing::info!(phase = "starting");
        for (index, plugin) in plugins.iter().enumerate() {
            plugin.start(self).await?;
            self.started_plugins
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            tracing::debug!(plugin = index, phase = "plugin-started");
        }

        // Ready commit: every registration and listener succeeded, so the
        // driver's barrier opens.
        self.ready.store(true, std::sync::atomic::Ordering::SeqCst);
        self.runtime.open_startup();
        tracing::info!(phase = "ready", "engine ready");

        // The host *is* the engine — its driver task was spawned in
        // `Runtime::create`, every port and the timer guider were attached
        // before the spawn, and the start barrier above kept it off the store
        // until now. There is nothing left to start here.

        Ok(())
    }

    /// Publish and register the packages this start carries: what the plugins
    /// registered in their `prepare`, the built-ins, then the builder's
    /// explicit packages (definition and runtime instance).
    async fn register_packages(&self, packages: Vec<ActPackageRegister>) -> crate::Result<()> {
        // Publishing a built-in package is the engine's own operation, not a
        // request: it runs as the unrestricted `system` principal, whatever
        // policy the deployment configured for its callers.
        let executor = self.executor(&Principal::unrestricted());

        // A plugin registers its package into the runtime registry from
        // `prepare`; at this point the registry holds exactly those packages,
        // so publish their definitions before the built-ins join it.
        for register in self.runtime.package().registers() {
            let meta = (register.meta)();
            executor.ext().register_package(&meta).await?;
        }

        package::init(self).await?;

        for package_register in packages {
            let meta = (package_register.meta)();
            executor.ext().register_package(&meta).await?;
            // Every run bound keeps its instance: `Func` executes it, and an
            // `Irq` act still asks it to prepare its params before the request
            // is sent to the client (`ActPackage::prepare_params`).
            self.runtime.package().register(meta.id, &package_register);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Clone)]
    struct FuncPackage;

    #[async_trait::async_trait]
    impl crate::ActPackage for FuncPackage {
        fn new(_: &Config) -> crate::Result<Self> {
            Ok(Self)
        }

        fn definition() -> crate::ActPackageDefinition {
            crate::ActPackageDefinition {
                id: "engine.test.func",
                name: "engine test func",
                desc: "",
                icon: "",
                doc: "",
                version: "0.1.0",
                schema: json!({}),
                options: None,
                run_as: crate::ActRunAs::Func,
                resources: vec![],
                catalog: crate::ActPackageCatalog::App,
            }
        }
    }

    /// A `Func` package passed through the builder is registered in the
    /// engine's own package registry, so its `uses` resolves at run time.
    #[tokio::test]
    async fn a_func_package_is_registered_by_the_engine() {
        let engine = Engine::builder()
            .add_package::<FuncPackage>()
            .start()
            .await
            .unwrap();
        assert!(engine.runtime().package().get("engine.test.func").is_some());
    }

    /// A run the engine never started has no unknown effects, and resolving
    /// one answers the missing aggregate instead of silently succeeding.
    #[tokio::test]
    async fn an_absent_run_has_no_unknown_effects() {
        let engine = Engine::builder().start().await.unwrap();
        assert!(
            engine
                .effect_unknowns("never-started")
                .await
                .unwrap()
                .is_empty()
        );
        let err = engine
            .resolve_effect("never-started", "k", EffectResolution::NotApplied)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not found"), "got: {err}");
    }

    /// A leftover `[acl]` table is ignored — the engine still starts, so a
    /// config written for the token era cannot brick an upgrade.
    #[tokio::test]
    async fn a_stale_acl_section_is_ignored() {
        let mut config = Config::default();
        config
            .table
            .insert("acl".to_string(), toml::Value::Table(toml::Table::new()));
        let engine = Engine::builder().set_config(&config).start().await.unwrap();
        assert!(engine.config().table.contains_key("acl"));
    }
}
