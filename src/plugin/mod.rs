use crate::{
    plugin::{org::OrgPlugin, role::RolePlugin},
    Engine,
};
use tracing::debug;

mod org;
mod role;

#[cfg(test)]
mod tests;

/// Act plugin trait
///
/// ## Example
///
/// ```rust
/// use acts::{ActPlugin, State, Message, Engine, Workflow};
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
///         // engine.register_action("func", func);
///         engine.emitter().on_start(|state: &State<Workflow>| {});
///         engine.emitter().on_complete(|state: &State<Workflow>| {});
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

fn register_plugins_default(plugins: &mut Vec<Box<dyn ActPlugin>>) {
    #[cfg(feature = "role")]
    plugins.push(Box::new(RolePlugin::new()));

    #[cfg(feature = "org")]
    plugins.push(Box::new(OrgPlugin::new()));
}
