use serde::de::DeserializeOwned;

use crate::{
    ActPackage, ActPlugin, ChannelOptions, Signal,
    builder::EngineBuilder,
    config::Config,
    export::{Channel, Executor, Extender},
    package::{self, ActPackageRegister},
    scheduler::Runtime,
};
use std::sync::Arc;

/// Workflow Engine
///
/// ## Example:
/// a example to caculate the result from 1 to given input value
///
///```rust,no_run
/// use acts::{Engine, Workflow, Vars};
///
/// #[tokio::main]
/// async fn main() {
///     let engine = Engine::new().start().unwrap();
///
///     let model = include_str!("../../../examples/simple/model.yml");
///     let workflow = Workflow::from_yml(model).unwrap();
///     
///     engine.channel().on_complete(|e| {
///         println!("{:?}", e.outputs);
///     });
///     let exec = engine.executor();
///     exec.model().deploy(&workflow, None).expect("fail to deploy workflow");
///
///     let mut vars = Vars::new();
///     vars.insert("input".into(), 3.into());
///     vars.insert("pid".into(), "test1".into());
///     exec.proc().start(
///        &workflow.id,
///        vars);
/// }
/// ```
#[derive(Clone)]
pub struct Engine {
    config: Arc<Config>,
    plugins: Vec<Arc<dyn ActPlugin>>,
    packages: Vec<ActPackageRegister>,
    runtime: Option<Arc<Runtime>>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        Self {
            config: Arc::new(Config::default()),
            plugins: Vec::new(),
            packages: Vec::new(),
            runtime: None,
        }
    }

    pub fn config(&self) -> Arc<Config> {
        self.config.clone()
    }

    pub fn with_config(mut self, config: &Config) -> Self {
        self.config = Arc::new(config.clone());
        self
    }

    /// register plugin
    ///
    /// ## Example
    ///
    /// ```no_run
    /// use acts::{ActPlugin, Message, Engine, Workflow, Result};
    ///
    /// #[derive(Clone)]
    /// struct TestPlugin;
    /// impl TestPlugin {
    ///     fn new() -> Self {
    ///         Self
    ///     }
    /// }
    /// #[async_trait::async_trait]
    /// impl ActPlugin for TestPlugin {
    ///     fn on_init(&self, engine: &Engine) -> Result<()> {
    ///         println!("TestPlugin");
    ///         engine.channel().on_start(|e| {});
    ///         engine.channel().on_complete(|e| {});
    ///         engine.channel().on_message(|e| {});
    ///         Ok(())       
    ///     }
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let engine = Engine::builder().add_plugin(&TestPlugin::new()).build().start().unwrap();
    /// }
    /// ```
    pub fn add_plugin<T>(mut self, plugin: &T) -> Self
    where
        T: ActPlugin + Clone + 'static,
    {
        self.plugins.push(Arc::new(plugin.clone()));
        self
    }

    pub fn set_plugins(mut self, plugins: Vec<Arc<dyn ActPlugin>>) -> Self {
        self.plugins = plugins;
        self
    }

    pub fn add_package<T>(mut self) -> Self
    where
        T: ActPackage + Clone + DeserializeOwned + 'static,
    {
        let package_register = ActPackageRegister::new::<T>();
        self.packages.push(package_register);
        self
    }

    pub fn set_packages(mut self, packages: Vec<ActPackageRegister>) -> Self {
        self.packages = packages;
        self
    }

    /// engine executor
    pub fn executor(&self) -> Arc<Executor> {
        Arc::new(Executor::new(&self.runtime()))
    }

    /// event channel (default to not support re-send)
    pub fn channel(&self) -> Arc<Channel> {
        Arc::new(Channel::new(&self.runtime()))
    }

    /// create named channel to receive messages
    /// if setting the emit_id by [`ChannelOptions`] it will check the status and re-send when not acking
    /// # Example
    /// ```no_run
    /// use acts::{ Engine, ChannelOptions };
    ///
    /// let engine = Engine::new().start().unwrap();
    /// let chan = engine.channel_with_options(&ChannelOptions {  
    ///     id: "chan1".to_string(),  
    ///     ack: true,  
    ///     r#type: "step".to_string(),
    ///     state: "{created, completed}".to_string(),
    ///     uses: "my_package".to_string(),
    ///     ..Default::default()
    /// });
    /// chan.on_message(|e| {
    ///     // do something
    /// });
    /// ```
    pub fn channel_with_options(&self, matcher: &ChannelOptions) -> Arc<Channel> {
        Arc::new(Channel::channel(&self.runtime(), matcher))
    }

    /// engine extender
    pub fn extender(&self) -> Arc<Extender> {
        Arc::new(Extender::new(&self.runtime()))
    }

    /// create engine builder
    pub fn builder() -> EngineBuilder {
        EngineBuilder::new()
    }

    pub(crate) fn runtime(&self) -> Arc<Runtime> {
        self.runtime.clone().expect("runtime not initialized")
    }

    /// close engine
    ///
    /// ## Example
    ///
    /// ```rust,no_run
    /// use acts::{Engine, Workflow, Vars};
    /// #[tokio::main]
    /// async fn main() {
    ///     let engine = Engine::new().start().unwrap();
    ///     engine.close();
    /// }
    /// ```
    pub fn close(&self) {
        self.runtime().close();
    }

    pub fn signal<T: Clone>(&self, init: T) -> Signal<T> {
        Signal::new(init)
    }

    pub fn start(mut self) -> crate::Result<Self> {
        self.runtime = Some(Runtime::new(&self.config())?);

        for plugin in self.plugins.iter() {
            plugin.on_init(&self)?;
        }

        // init built-in packages
        package::init(&self)?;

        // register packages
        for package_register in self.packages.iter() {
            let meta = (package_register.meta)();
            self.extender().register_package(&meta)?;
            if meta.run_as == crate::ActRunAs::Func {
                self.runtime().package().register(meta.id, package_register);
            }
        }

        // start event loop
        self.runtime().event_loop();

        Ok(self)
    }
}
