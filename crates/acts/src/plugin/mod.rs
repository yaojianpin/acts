use crate::Engine;

mod transport;
pub use transport::{
    InflightCounter, InflightGuard, RunningTransport, ShutdownConfig, TransportDrainReport,
};

#[cfg(test)]
mod tests;

/// Act plugin trait
///
/// A plugin has three phases, in this order:
///
/// 1. [`prepare`](Self::prepare) — validate configuration and register what it
///    provides (channel handlers, packages). No listener is bound and no task
///    is spawned: a failed prepare leaves nothing behind.
/// 2. [`start`](Self::start) — bind listeners and spawn tasks. Called only
///    after **every** plugin prepared, and before the scheduler's start
///    barrier opens.
/// 3. [`shutdown`](Self::shutdown) — stop accepting work and join the tasks.
///    Called in reverse start order on close and on a failed start.
///
/// ## Example
///
/// ```rust,no_run
/// use acts::{ActPlugin, Result, Engine, Workflow};
/// #[derive(Clone)]
/// struct TestPlugin;
///
/// impl TestPlugin {
///     fn new() -> Self {
///         Self
///     }
/// }
///
/// #[async_trait::async_trait]
/// impl ActPlugin for TestPlugin {
///     async fn prepare(&self, engine: &Engine) -> Result<()> {
///         println!("TestPlugin");
///         // engine.register_module("name", module);
///         engine.channel().on_start(|_| async {});
///         engine.channel().on_complete(|_| async {});
///         engine.channel().on_message(|_| async {});
///         Ok(())
///     }
/// }
/// ```
#[async_trait::async_trait]
pub trait ActPlugin: Send + Sync {
    /// Validate the plugin's configuration and register what it provides.
    ///
    /// This runs for every plugin before any listener starts, so it must not
    /// bind a socket, subscribe to a broker or spawn a long-running task: a
    /// failure here aborts the start with nothing to undo.
    async fn prepare(&self, _engine: &Engine) -> crate::Result<()> {
        Ok(())
    }

    /// Bind listeners and spawn the plugin's tasks.
    ///
    /// Runs only after every plugin prepared. A failure shuts down the plugins
    /// already started (in reverse order) and aborts the engine start.
    async fn start(&self, _engine: &Engine) -> crate::Result<()> {
        Ok(())
    }

    /// Stop accepting work and join the plugin's tasks.
    ///
    /// Called in reverse start order by [`Engine::close`] and by a failed
    /// start's rollback. The engine's shutdown token is already cancelled when
    /// this runs; the call must be idempotent and should not block forever.
    async fn shutdown(&self) -> crate::Result<()> {
        Ok(())
    }
}
