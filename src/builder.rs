use crate::{ActPlugin, Config, Engine};

pub struct EngineBuilder {
    config: Config,
    plugins: Vec<Box<dyn ActPlugin>>,
}

impl Default for EngineBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl EngineBuilder {
    pub fn new() -> Self {
        Self {
            config: Config::default(),
            plugins: Vec::new(),
        }
    }

    pub fn set_config(&mut self, config: &Config) {
        self.config = config.clone();
    }

    pub fn log_dir(mut self, log_dir: &str) -> Self {
        self.config.log_dir = log_dir.to_string();
        self
    }

    pub fn log_level(mut self, level: &str) -> Self {
        self.config.log_level = level.to_string();
        self
    }

    pub fn cache_size(mut self, size: usize) -> Self {
        self.config.cache_cap = size;
        self
    }

    pub fn database_url(mut self, url: &str) -> Self {
        self.config.database_url = Some(url.to_string());
        self
    }

    pub fn tick_interval_secs(mut self, secs: u64) -> Self {
        self.config.tick_interval_secs = secs;
        self
    }

    pub fn max_message_retry_times(mut self, retry_times: i32) -> Self {
        self.config.max_message_retry_times = retry_times;
        self
    }

    /// register plugin
    ///
    /// ## Example
    ///
    /// ```no_run
    /// use acts::{ActPlugin, Message, Engine, EngineBuilder, Workflow};
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
    /// let engine = EngineBuilder::new().add_plugin(&TestPlugin::new()).build().start();
    /// ```
    pub fn add_plugin<T>(mut self, plugin: &T) -> Self
    where
        T: ActPlugin + Clone + 'static,
    {
        self.plugins.push(Box::new(plugin.clone()));
        self
    }

    pub fn build(&self) -> Engine {
        let engine = Engine::new_with_config(&self.config);

        // init plugins
        for plugin in self.plugins.iter() {
            plugin.on_init(&engine);
        }

        engine
    }
}
