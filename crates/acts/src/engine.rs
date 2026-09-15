use crate::{
    Acl, AclConfig, ActPlugin, ChannelOptions, Config, Principal, Signal,
    builder::EngineBuilder,
    export::{Channel, Executor},
    package::{self, ActPackageRegister},
    scheduler::Runtime,
    snapshot::{SnapshotManager, SnapshotOptions},
};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use serde::Deserialize;
/// A started workflow engine.
///
/// An `Engine` is created only by [`EngineBuilder::start`]. Its runtime is
/// therefore always initialized.
#[derive(Clone)]
pub struct Engine {
    config: Arc<Config>,
    runtime: Arc<Runtime>,
    acl: Arc<Acl>,
}

impl Engine {
    pub fn builder() -> EngineBuilder {
        EngineBuilder::new()
    }

    pub fn config(&self) -> Arc<Config> {
        self.config.clone()
    }

    /// The compiled access control policy. Without an `[acl]` section this is
    /// a disabled ACL and every check passes.
    pub fn acl(&self) -> Arc<Acl> {
        self.acl.clone()
    }

    /// The principal a caller that presents no token resolves to — the
    /// identity to hand [`Engine::executor`] on behalf of such a request.
    ///
    /// Without an `[acl]` section this is the read-only `anonymous` subject;
    /// with one it is the configured `default_role`, or a principal that is
    /// refused everything when no default role exists.
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

    pub(crate) fn with_runtime(config: Arc<Config>, runtime: Arc<Runtime>) -> crate::Result<Self> {
        let acl = match config.table.get("acl") {
            Some(value) => {
                let acl_config = AclConfig::deserialize(value.clone()).map_err(|err| {
                    crate::ActError::Config(format!("failed to parse the 'acl' config: {err}"))
                })?;
                Acl::from_config(&acl_config)?
            }
            // No section is a deployment that has not said who may do what:
            // it answers to anyone, with the anonymous read-only policy.
            None => {
                tracing::warn!(
                    "no [acl] section in the config: callers are anonymous and read-only. Add [acl] with a token to grant more, or set enabled = false to lift the limits."
                );
                Acl::anonymous_access()
            }
        };

        Ok(Self {
            config,
            runtime,
            acl: Arc::new(acl),
        })
    }

    pub(crate) async fn initialize(
        &self,
        snapshots: Vec<(String, SnapshotOptions)>,
        plugins: Vec<Arc<dyn ActPlugin>>,
        packages: Vec<ActPackageRegister>,
    ) -> crate::Result<()> {
        self.prepare(snapshots, plugins, packages).await?;

        // Start the event loop only after plugins and packages have registered
        // their channels and handlers.
        self.runtime.event_loop();

        // Outbox replay first: every task that has a durable pending record is
        // driven deterministically to its next checkpoint (NEXT_COMPLETE /
        // applied-action guards make the replay idempotent); resume runs after
        // so it only sees what the replay left mid-flight and never overlaps
        // the replay on the same task.
        self.runtime.recover_actions().await?;

        // Resume in-flight processes (durable Ready/Running/Pending rows) and
        // start parked ones.
        self.runtime.resume().await?;

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
