use crate::Engine;
use tracing::debug;

#[cfg(test)]
mod tests;

/// Act plugin trait
///
/// ## Example
///
/// ```rust
/// use acts::{ActPlugin, WorkflowState, Message, Engine, Workflow};
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
///         engine.emitter().on_start(|state: &WorkflowState| {});
///         engine.emitter().on_complete(|state: &WorkflowState| {});
///         engine.emitter().on_message(|msg: &Message| {});
///     }
/// }
/// ```
pub trait ActPlugin: Send + Sync {
    fn on_init(&self, engine: &Engine);
}

pub fn init(engine: &Engine) {
    debug!("plugin::init");
    let extender = engine.extender();
    let mut plugins = &mut *extender.plugins.lock().unwrap();

    register_plugins_default(&mut plugins);
    for plugin in plugins.into_iter() {
        plugin.on_init(engine);
    }
}

fn register_plugins_default(_plugins: &mut Vec<Box<dyn ActPlugin>>) {}
