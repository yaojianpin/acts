use serde::de::DeserializeOwned;

use crate::{
    ActPackage, ActPlugin, ChannelOptions, Signal,
    builder::EngineBuilder,
    config::{Config, ConfigResolver},
    export::{Channel, Executor, Extender},
    package::{self, ActPackageRegister},
    scheduler::Runtime,
    store::KvStore,
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
///     let engine = Engine::new().start().await.unwrap();
///
///     let model = include_str!("../../../examples/simple/model.yml");
///     let workflow = Workflow::from_yml(model).unwrap();
///     
///     engine.channel().on_complete(|e| async move {
///         println!("{:?}", e.outputs);
///     });
///     let exec = engine.executor();
///     exec.model().deploy(&workflow, None).await.expect("fail to deploy workflow");
///
///     let mut vars = Vars::new();
///     vars.insert("input".into(), 3.into());
///     vars.insert("pid".into(), "test1".into());
///     exec.proc().start(&workflow.id, vars).await.unwrap();
/// }
/// ```
#[derive(Clone)]
pub struct Engine {
    config: Arc<Config>,
    plugins: Vec<Arc<dyn ActPlugin>>,
    packages: Vec<ActPackageRegister>,
    resolvers: Vec<(String, Arc<dyn ConfigResolver>)>,
    store: Option<Arc<dyn KvStore>>,
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
            resolvers: Vec::new(),
            store: None,
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
    ///         engine.channel().on_start(|_| async {});
    ///         engine.channel().on_complete(|_| async {});
    ///         engine.channel().on_message(|_| async {});
    ///         Ok(())       
    ///     }
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let engine = Engine::builder().add_plugin(&TestPlugin::new()).build().start().await.unwrap();
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

    /// Register a named config resolver. Called by plugins in `on_init`,
    /// or via [`EngineBuilder::add_resolver`] before starting the engine.
    ///
    /// Resolvers are invoked at `proc.start()` on each task to inject
    /// tenant-scoped configuration into [`sealed_data`](crate::task::Task::sealed),
    /// which inherits from parent tasks.
    pub fn add_resolver(&self, name: &str, resolver: Arc<dyn ConfigResolver>) {
        self.runtime().register_resolver(name, resolver);
    }

    pub fn set_packages(mut self, packages: Vec<ActPackageRegister>) -> Self {
        self.packages = packages;
        self
    }
    pub fn set_resolvers(mut self, resolvers: Vec<(String, Arc<dyn ConfigResolver>)>) -> Self {
        self.resolvers = resolvers;
        self
    }
    pub fn set_store(mut self, store: Option<Arc<dyn KvStore>>) -> Self {
        self.store = store;
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
    /// use acts::{Engine, ChannelOptions};
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let engine = Engine::new().start().await.unwrap();
    ///     let chan = engine.channel_with_options(&ChannelOptions {
    ///         id: "chan1".to_string(),
    ///         ack: true,
    ///         r#type: "step".to_string(),
    ///         state: "{created, completed}".to_string(),
    ///         uses: "my_package".to_string(),
    ///         ..Default::default()
    ///     });
    ///     chan.on_message(|_| async {
    ///         // do something
    ///     });
    /// }
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
    /// use acts::Engine;
    /// #[tokio::main]
    /// async fn main() {
    ///     let engine = Engine::new().start().await.unwrap();
    ///     engine.close().await;
    /// }
    /// ```
    pub async fn close(&self) {
        self.runtime().close().await;
    }

    pub fn signal<T: Clone>(&self, init: T) -> Signal<T> {
        Signal::new(init)
    }

    pub async fn start(mut self) -> crate::Result<Self> {
        self.runtime = Some(Runtime::new(&self.config(), self.store.clone())?);

        let rt = self.runtime();

        // Any failure below must not leave the partially started runtime
        // behind: the store writer task, the event loop, the recovery writes
        // and (when reached) the retry/trigger timer tasks would keep running
        // on an engine that never became usable.
        let init = (|| async {
            self.prepare().await?;

            // start event loop
            rt.event_loop();

            // recover pending actions
            rt.recover_actions().await?;

            // init retry timer
            rt.init_retry_timer()?;

            // schedule trigger timer
            rt.init_trigger_timer();

            Ok::<_, crate::ActError>(())
        })()
        .await;

        if let Err(err) = init {
            rt.close().await;
            self.runtime = None;
            return Err(err);
        }

        info!("engine started");

        Ok(self)
    }

    async fn prepare(&self) -> crate::Result<()> {
        // register resolvers
        for (name, resolver) in self.resolvers.iter() {
            self.runtime().register_resolver(name, resolver.clone());
        }

        // init plugins
        for plugin in self.plugins.iter() {
            plugin.on_init(self)?;
        }

        // init built-in packages
        package::init(self).await?;

        // register packages
        for package_register in self.packages.iter() {
            let meta = (package_register.meta)();
            self.extender().register_package(&meta).await?;
            if meta.run_as == crate::ActRunAs::Func {
                self.runtime().package().register(meta.id, package_register);
            }
        }

        Ok(())
    }
}
