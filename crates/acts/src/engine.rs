use crate::{
    AccessControl, ActPlugin, ChannelOptions, Config, Principal, Signal, Vars,
    builder::EngineBuilder,
    export::{Channel, Executor},
    package::{self, ActPackageRegister},
    scheduler::Runtime,
    snapshot::{SnapshotManager, SnapshotOptions},
};
use std::sync::Arc;
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
}

impl Engine {
    pub fn builder() -> EngineBuilder {
        EngineBuilder::new()
    }

    pub fn config(&self) -> Arc<Config> {
        self.config.clone()
    }

    /// The engine's access control. A bare engine runs [`AnonymousAcl`]: no
    /// users, no sessions, the anonymous catalogue-only policy for every
    /// caller — install `acts-acl`'s `UserAcl` (see
    /// [`EngineBuilder::set_acl`]) for users, login and sessions.
    pub fn acl(&self) -> Arc<dyn AccessControl> {
        self.acl.clone()
    }

    /// The principal a caller that presents no token resolves to — the
    /// identity to hand [`Engine::executor`] on behalf of such a request.
    ///
    /// It is the anonymous principal: the catalogue reads, and nothing else
    /// (a disabled ACL resolves it to [`Principal::unrestricted`] instead).
    pub fn anonymous(&self) -> Principal {
        self.acl.anonymous()
    }

    /// Register (or replace) a snapshot-backed sealed-data target at runtime.
    /// Returns an error when the options are invalid (see
    /// [`SnapshotOptions::validate`]). Prefer [`EngineBuilder::add_snapshot`]
    /// when the options are known before starting.
    pub fn add_snapshot(&self, name: &str, options: SnapshotOptions) -> crate::Result<()> {
        self.runtime.register_snapshot(name, options)?;
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
        self.runtime.kernel().effect_unknowns(pid).await
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
            .kernel()
            .resolve_effect_by_key(pid, key, resolution)
            .await
    }

    pub(crate) fn runtime(&self) -> Arc<Runtime> {
        self.runtime.clone()
    }

    /// Close the engine and stop its runtime.
    pub async fn close(&self) {
        self.runtime.close().await;
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
        })
    }

    pub(crate) async fn initialize(
        &self,
        snapshots: Vec<(String, SnapshotOptions)>,
        plugins: Vec<Arc<dyn ActPlugin>>,
        packages: Vec<ActPackageRegister>,
    ) -> crate::Result<()> {
        // The access control loads before any plugin starts a transport: its
        // users (and the builtin admin, if it has one) exist by the time the
        // first request arrives.
        self.acl.load(self.runtime.store()).await?;

        self.prepare(snapshots, plugins, packages).await?;

        // Boot. The host *is* the engine — its driver task is already running
        // (spawned in `Runtime::create`) and it recovers every pending
        // operation on its own — so only the periodic timers start here. The
        // retry timer re-sends unacknowledged deliveries; the cleanup timer
        // reclaims terminal runs on its own budget and health; the trigger
        // timer fires due schedules; the snapshot timer drops expired entries.
        self.runtime.init_retry_timer()?;
        self.runtime.init_cleanup_timer();
        self.runtime.init_trigger_timer();
        self.runtime.init_snapshot_timer();

        Ok(())
    }
    async fn prepare(
        &self,
        snapshots: Vec<(String, SnapshotOptions)>,
        plugins: Vec<Arc<dyn ActPlugin>>,
        packages: Vec<ActPackageRegister>,
    ) -> crate::Result<()> {
        // Register snapshot targets (data feeds come from plugins/adapters).
        for (name, options) in snapshots {
            self.runtime.register_snapshot(&name, options)?;
        }

        for plugin in plugins {
            plugin.on_init(self)?;
        }

        package::init(self).await?;

        // Publishing a built-in package is the engine's own operation, not a
        // request: it runs as the unrestricted `system` principal, whatever
        // policy the deployment configured for its callers.
        let executor = self.executor(&Principal::unrestricted());
        for package_register in packages {
            let meta = (package_register.meta)();
            executor.ext().register_package(&meta).await?;
            if meta.run_as == crate::ActRunAs::Func {
                self.runtime.package().register(meta.id, &package_register);
            }
        }

        Ok(())
    }
}
