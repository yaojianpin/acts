use crate::Engine;

#[cfg(test)]
mod tests;

/// Act plugin trait
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
///     async fn on_init(&self, engine: &Engine) -> Result<()> {
///         println!("TestPlugin");
///         // engine.register_module("name", module);
///         engine.channel().on_start(|e| {});
///         engine.channel().on_complete(|e| {});
///         engine.channel().on_message(|e| {});
///         Ok(())
///     }
/// }
/// ```
#[async_trait::async_trait]
pub trait ActPlugin: Send + Sync {
    async fn on_init(&self, engine: &Engine) -> crate::Result<()>;
}
