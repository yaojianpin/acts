use crate::{
    ChannelOptions, Signal,
    config::Config,
    export::{Channel, Executor, Extender},
    package,
    scheduler::Runtime,
};

use std::sync::Arc;
use tracing::info;

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
///     let engine = Engine::new().start();
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
///        &vars);
/// }
/// ```
#[derive(Clone)]
pub struct Engine {
    runtime: Arc<Runtime>,
    extender: Arc<Extender>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        Self::new_with_config(&Config::default())
    }

    pub fn config(&self) -> Arc<Config> {
        self.runtime.config().clone()
    }

    /// engine executor
    pub fn executor(&self) -> Arc<Executor> {
        Arc::new(Executor::new(&self.runtime))
    }

    /// event channel (default to not support re-send)
    pub fn channel(&self) -> Arc<Channel> {
        Arc::new(Channel::new(&self.runtime))
    }

    /// create named channel to receive messages
    /// if setting the emit_id by [`ChannelOptions`] it will check the status and re-send when not acking
    /// # Example
    /// ```no_run
    /// use acts::{ Engine, ChannelOptions };
    ///
    /// let engine = Engine::new().start();
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
        Arc::new(Channel::channel(&self.runtime, matcher))
    }

    /// engine extender
    pub fn extender(&self) -> Arc<Extender> {
        self.extender.clone()
    }

    pub(crate) fn runtime(&self) -> Arc<Runtime> {
        self.runtime.clone()
    }

    /// close engine
    ///
    /// ## Example
    ///
    /// ```rust,no_run
    /// use acts::{Engine, Workflow, Vars};
    /// #[tokio::main]
    /// async fn main() {
    ///     let engine = Engine::new().start();
    ///     engine.close();
    /// }
    /// ```
    pub fn close(&self) {
        info!("close");
        self.runtime.scher().close();
    }

    pub fn signal<T: Clone>(&self, init: T) -> Signal<T> {
        Signal::new(init)
    }

    pub fn is_running(&self) -> bool {
        self.runtime.is_running()
    }

    fn init(&self) {
        info!("init");
        self.runtime.init(self);
        package::init(self);
    }

    pub(crate) fn new_with_config(config: &Config) -> Self {
        info!("config: {:?}", config);
        let runtime = Runtime::new(config);

        let extender = Arc::new(Extender::new(&runtime));
        Self { runtime, extender }
    }

    pub fn start(self) -> Self {
        self.init();
        self
    }
}
