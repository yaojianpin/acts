#[cfg(test)]
use crate::ConfigData;
use crate::{ActPlugin, Config, Engine, Result};
use std::path::Path;

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
        let mut config = Config::default();
        #[cfg(not(test))]
        let file = Path::new("config/acts.cfg");

        #[cfg(test)]
        let file = Path::new("test/acts.cfg");

        if file.exists() {
            config = Config::create(file);
        }

        Self {
            config,
            plugins: Vec::new(),
        }
    }

    #[cfg(test)]
    pub fn set_config(mut self, data: &ConfigData) -> Self {
        self.config = Config {
            data: data.clone(),
            table: hocon::Hocon::Null,
        };
        self
    }

    pub fn set_config_source(mut self, source: &Path) -> Self {
        self.config = Config::create(source);
        self
    }

    pub fn log_dir(mut self, log_dir: &str) -> Self {
        self.config.data.log_dir = log_dir.to_string();
        self
    }

    pub fn log_level(mut self, level: &str) -> Self {
        self.config.data.log_level = level.to_string();
        self
    }

    pub fn cache_size(mut self, size: i64) -> Self {
        self.config.data.cache_cap = size;
        self
    }

    pub fn tick_interval_secs(mut self, secs: i64) -> Self {
        self.config.data.tick_interval_secs = secs;
        self
    }

    pub fn max_message_retry_times(mut self, retry_times: i32) -> Self {
        self.config.data.max_message_retry_times = retry_times;
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
    ///     async fn on_init(&self, engine: &Engine) -> Result<()> {
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
    ///     let engine = EngineBuilder::new().add_plugin(&TestPlugin::new()).build().await.unwrap().start();
    /// }
    /// ```
    pub fn add_plugin<T>(mut self, plugin: &T) -> Self
    where
        T: ActPlugin + Clone + 'static,
    {
        self.plugins.push(Box::new(plugin.clone()));
        self
    }

    pub async fn build(&self) -> Result<Engine> {
        let engine = Engine::new_with_config(&self.config);

        // init the cache store to make sure the plugin can registry package to the store
        engine.runtime().cache().init(&engine);

        // init plugins
        for plugin in self.plugins.iter() {
            plugin.on_init(&engine).await?;
        }

        Ok(engine)
    }
}
