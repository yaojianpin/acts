use core::fmt;
use std::sync::{Arc, Mutex};

use crate::{scheduler::Runtime, ActModule, ActPlugin};

#[derive(Clone)]
pub struct Extender {
    runtime: Arc<Runtime>,
    plugins: Arc<Mutex<Vec<Box<dyn ActPlugin>>>>,
}

impl fmt::Debug for Extender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Extender").finish()
    }
}

impl Extender {
    pub(crate) fn new(runtime: &Arc<Runtime>) -> Self {
        Self {
            runtime: runtime.clone(),
            plugins: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// register module
    ///
    /// ## Example
    /// ```no_run
    /// use acts::Engine;
    /// mod test_module {
    ///   use acts::{ActModule, Result};
    ///   #[derive(Clone)]
    ///   pub struct TestModule;
    ///   impl ActModule for TestModule {
    ///     fn init<'a>(&self, _ctx: &rquickjs::Ctx<'a>) -> Result<()> {
    ///         Ok(())
    ///     }
    ///   }
    /// }
    /// let engine = Engine::new();
    /// let module = test_module::TestModule;
    /// engine.extender().register_module(&module);
    /// ```
    pub fn register_module<T: ActModule + Clone + 'static>(&self, module: &T) {
        self.runtime.env().register_module(module)
    }

    /// register plugin
    ///
    /// ## Example
    ///
    /// ```no_run
    /// use acts::{ActPlugin, Message, Engine, Workflow};
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
    ///         engine.channel().on_start(|e| {});
    ///         engine.channel().on_complete(|e| {});
    ///         engine.channel().on_message(|e| {});
    ///     }
    /// }
    /// let engine = Engine::new();
    /// engine.extender().register_plugin(&TestPlugin::new());
    /// ```
    pub fn register_plugin<T: ActPlugin + 'static + Clone>(&self, plugin: &T) {
        let mut plugins = self.plugins.lock().unwrap();
        plugins.push(Box::new(plugin.clone()));
    }

    pub fn plugins(&self) -> Arc<Mutex<Vec<Box<dyn ActPlugin>>>> {
        self.plugins.clone()
    }
}
