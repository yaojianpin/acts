use crate::{
    AccessControl, ActPlugin, ChannelOptions, Config, Principal, Signal,
    builder::EngineBuilder,
    export::{Channel, Executor},
    package::{self, ActPackageRegister},
    scheduler::Runtime,
    snapshot::{SnapshotManager, SnapshotOptions},
};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

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
        self.acl.load(self.runtime.cache().store()).await?;

        self.prepare(snapshots, plugins, packages).await?;

        // Start the event loop only after plugins and packages have registered
        // their channels and handlers.
        self.runtime.event_loop();

        self.runtime.resume().await?;

        // Outbox replay: every task that has a durable pending record is
        // driven deterministically to its next checkpoint (applied propagation
        // / applied-action guards make the replay idempotent). It runs after
        // the resume so the two never overlap on the same task: the resume
        // re-dispatches only tasks without a pending record and leaves every
        // record to this replay.
        self.runtime.recover_actions().await?;

        self.runtime.init_retry_timer()?;
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
