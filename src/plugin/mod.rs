use crate::Engine;
use tracing::debug;

#[cfg(test)]
mod tests;

/// Act plugin trait
///
/// ## Example
///
/// ```rust,no_run
/// use acts::{ActPlugin, Engine, Workflow};
/// #[derive(Clone)]
/// struct TestPlugin;
///
/// impl TestPlugin {
///     fn new() -> Self {
///         Self
///     }
/// }
///
/// impl ActPlugin for TestPlugin {
///     fn on_init(&self, engine: &Engine) {
///         println!("TestPlugin");
///         // engine.register_module("name", module);
///         engine.emitter().on_start(|e| {});
///         engine.emitter().on_complete(|e| {});
///         engine.emitter().on_message(|e| {});
///     }
/// }
/// ```
pub trait ActPlugin: Send + Sync {
    fn on_init(&self, engine: &Engine);
}

pub fn init(engine: &Engine) {
    debug!("plugin::init");
    let plugins = engine.plugins();
    let plugins = plugins.lock().unwrap();
    for plugin in plugins.iter() {
        plugin.on_init(engine);
    }
}
