use std::sync::{Arc, Mutex};

use crate::{env::Enviroment, ActModule, ActPlugin};

#[derive(Clone)]
pub struct Extender {
    env: Arc<Enviroment>,
    plugins: Arc<Mutex<Vec<Box<dyn ActPlugin>>>>,
}

impl Extender {
    pub(crate) fn new(
        env: &Arc<Enviroment>,
        plugins: &Arc<Mutex<Vec<Box<dyn ActPlugin>>>>,
    ) -> Self {
        Self {
            env: env.clone(),
            plugins: plugins.clone(),
        }
    }

    /// register module
    ///
    /// ## Example
    /// ```rust
    /// #[tokio::test]
    /// async fn engine_register_module() {
    ///     let engine = Engine::new();
    ///     let mut module = Module::new();
    ///     combine_with_exported_module!(&mut module, "role", test_module);
    ///     engine.mgr().register_module("test", &module);
    ///     assert!(engine.mgr().modules().contains_key("test"));
    /// }
    /// ```
    pub fn register_module<T: ActModule + Clone + 'static>(&self, module: &T) {
        self.env.register_module(module)
    }

    /// register plugin
    ///
    /// ## Example
    ///
    /// ```no_run
    /// use acts::{ActPlugin, WorkflowState, Message, Engine, Workflow};
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
    ///         // engine.extender().register_module("name", module);
    ///         engine.emitter().on_start(|e| {});
    ///         engine.emitter().on_complete(|e| {});
    ///         engine.emitter().on_message(|e| {});
    ///     }
    /// }
    /// let engine = Engine::new();
    /// engine.extender().register_plugin(&TestPlugin::new());
    /// ```
    pub fn register_plugin<T: ActPlugin + 'static + Clone>(&self, plugin: &T) {
        let mut plugins = self.plugins.lock().unwrap();
        plugins.push(Box::new(plugin.clone()));
    }
}
