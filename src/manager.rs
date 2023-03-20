use crate::{ActModule, ActPlugin};
use rhai::{EvalAltResult, Identifier, RegisterNativeFunction, Variant};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[derive(Clone)]
pub struct Manager {
    action: Arc<Mutex<ActModule>>,
    modules: Arc<Mutex<HashMap<String, ActModule>>>,
    pub(crate) plugins: Arc<Mutex<Vec<Box<dyn ActPlugin>>>>,
}

impl Manager {
    pub(crate) fn new() -> Self {
        Self {
            plugins: Arc::new(Mutex::new(Vec::new())),
            action: Arc::new(Mutex::new(ActModule::new())),
            modules: Arc::new(Mutex::new(HashMap::new())),
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
    pub fn register_module(&self, name: &str, module: &ActModule) {
        self.modules
            .lock()
            .unwrap()
            .insert(name.to_string(), module.clone());
    }

    /// register act function
    ///
    /// ## Example
    ///
    /// ```rust
    /// #[tokio::test]
    /// async fn engine_register_module() {
    ///     let mut engine = Engine::new();
    ///     let add = |a: i64, b: i64| Ok(a + b);
    ///     engine.register_action("add", add);
    /// }
    /// ```
    pub fn register_action<A: 'static, const N: usize, const C: bool, T, F>(
        &self,
        name: impl AsRef<str> + Into<Identifier>,
        func: F,
    ) -> u64
    where
        T: Variant + Clone,
        F: RegisterNativeFunction<A, N, C, T, true>,
    {
        self.action.lock().unwrap().set_native_fn(name, func)
    }

    /// register plugin
    ///
    /// ## Example
    ///
    /// ```no_run
    /// use acts::{ActPlugin, State, Message, Engine, Workflow};
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
    ///         // engine.mgr().register_module("name", module);
    ///         // engine.mgr().register_action("func", func);
    ///         engine.emitter().on_start(|state: &State<Workflow>| {});
    ///         engine.emitter().on_complete(|state: &State<Workflow>| {});
    ///         engine.emitter().on_message(|_msg: &Message| {});
    ///     }
    /// }
    /// let engine = Engine::new();
    /// engine.mgr().register_plugin(&TestPlugin::new());
    /// ```
    pub fn register_plugin<T: ActPlugin + 'static + Clone>(&self, plugin: &T) {
        self.plugins.lock().unwrap().push(Box::new(plugin.clone()));
    }

    pub(crate) fn modules(&self) -> HashMap<String, ActModule> {
        self.modules.lock().unwrap().clone()
    }

    pub(crate) fn action(&self) -> ActModule {
        self.action.lock().unwrap().clone()
    }
}
