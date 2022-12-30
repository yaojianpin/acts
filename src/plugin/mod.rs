use crate::{
    debug,
    plugin::{org::OrgPlugin, role::RolePlugin},
    Engine,
};

mod org;
mod role;

#[cfg(test)]
mod tests;

/// Act plugin trait
///
/// ## Example
///
/// ```rust
/// use yao::{ActPlugin, Message, Engine, Workflow};
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
///         engine.on_workflow_start(|w: &Workflow| {});
///         engine.on_workflow_complete(|w: &Workflow| {});
///         engine.on_message(|msg: &Message| {});
///     }
/// }
/// ```
pub trait ActPlugin: std::fmt::Debug + Send + Sync {
    fn on_init(&self, engine: &Engine);
}

pub async fn init(engine: &Engine) {
    debug!("plugin::init");
    let mut plugins = &mut *engine.plugins.lock().unwrap();

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

impl Engine {
    /// register plugin
    ///
    /// ## Example
    ///
    /// ```no_run
    /// use yao::{ActPlugin, Message, Engine, Workflow};
    ///
    /// #[derive(Clone)]
    /// struct TestPlugin;
    /// impl TestPlugin {
    ///     fn new() -> Self {
    ///         Self
    ///     }
    /// }
    /// impl ActPlugin for TestPlugin {
    ///     fn on_init(&self, engine: &Engine) {
    ///         println!("TestPlugin");
    ///         // engine.register_module("name", module);
    ///         // engine.register_action("func", func);
    ///         engine.on_workflow_start(|_w: &Workflow| {});
    ///         engine.on_workflow_complete(|_w: &Workflow| {});
    ///         engine.on_message(|_msg: &Message| {});
    ///     }
    /// }
    /// let engine = Engine::new();
    /// engine.register_plugin(&TestPlugin::new());
    /// ```
    pub fn register_plugin<T: ActPlugin + 'static + Clone>(&self, plugin: &T) {
        self.plugins.lock().unwrap().push(Box::new(plugin.clone()));
    }
}
