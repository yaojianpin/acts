use crate::{
    ActPlugin, ChannelOptions, Signal,
    config::Config,
    export::{Channel, Executor, Extender},
    package,
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
///     exec.model().deploy(&workflow).expect("fail to deploy workflow");
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
    /// use acts::{ActPlugin, Message, Engine, EngineBuilder, Workflow, Result};
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
    ///     let engine = EngineBuilder::new().add_plugin(&TestPlugin::new()).build().start().unwrap();
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
    ///     key: "my_key*".to_string(),
    ///     state: "{created, completed}".to_string(),
    ///     uses: "my_package".to_string(),
    ///     tag: "*".to_string()  
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
        package::init(&self)?;

        // start event loop
        self.runtime().event_loop();

        Ok(self)
    }
}
