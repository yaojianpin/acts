use crate::{
    ActPlugin, ChannelOptions, Config, Signal,
    builder::EngineBuilder,
    export::{Channel, Executor, Extender},
    package::{self, ActPackageRegister},
    scheduler::Runtime,
    snapshot::{SnapshotManager, SnapshotOptions},
};
use std::sync::Arc;

/// A started workflow engine.
///
/// An `Engine` is created only by [`EngineBuilder::start`]. Its runtime is
/// therefore always initialized.
#[derive(Clone)]
pub struct Engine {
    config: Arc<Config>,
    runtime: Arc<Runtime>,
}

impl Engine {
    pub fn builder() -> EngineBuilder {
        EngineBuilder::new()
    }

    pub fn config(&self) -> Arc<Config> {
        self.config.clone()
    }

    /// Register (or replace) a snapshot-backed sealed-data target at runtime.
    /// Prefer [`EngineBuilder::add_snapshot`] when the options are known
    /// before starting.
    pub fn add_snapshot(&self, name: &str, options: SnapshotOptions) {
        self.runtime.register_snapshot(name, options);
    }

    /// Engine executor.
    pub fn executor(&self) -> Arc<Executor> {
        Arc::new(Executor::new(&self.runtime))
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

    /// Engine extender.
    pub fn extender(&self) -> Arc<Extender> {
        Arc::new(Extender::new(&self.runtime))
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

    pub fn signal<T: Clone>(&self, init: T) -> Signal<T> {
        Signal::new(init)
    }

    pub(crate) fn with_runtime(config: Arc<Config>, runtime: Arc<Runtime>) -> Self {
        Self { config, runtime }
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
            self.runtime.register_snapshot(&name, options);
        }

        for plugin in plugins {
            plugin.on_init(self)?;
        }

        package::init(self).await?;

        for package_register in packages {
            let meta = (package_register.meta)();
            self.extender().register_package(&meta).await?;
            if meta.run_as == crate::ActRunAs::Func {
                self.runtime.package().register(meta.id, &package_register);
            }
        }

        Ok(())
    }
}
