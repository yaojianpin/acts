#[cfg(test)]
use crate::config::ConfigData;
use crate::{ActPlugin, Config, Engine, config::ConfigLog};
use std::{path::Path, sync::Arc};

pub struct EngineBuilder {
    config: Config,
    plugins: Vec<Arc<dyn ActPlugin>>,
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
        let file = Path::new("config/acts.toml");

        #[cfg(test)]
        let file = Path::new("test/acts.toml");

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
            table: toml::Table::new(),
        };
        self
    }

    pub fn set_config_source(mut self, source: &Path) -> Self {
        self.config = Config::create(source);
        self
    }

    pub fn log(mut self, dir: &str, level: &str) -> Self {
        self.config.data.log = Some(ConfigLog {
            dir: dir.to_string(),
            level: level.to_string(),
        });
        self
    }

    pub fn cache_size(mut self, size: i64) -> Self {
        self.config.data.cache_cap = Some(size);
        self
    }

    pub fn tick_interval_secs(mut self, secs: i64) -> Self {
        self.config.data.tick_interval_secs = Some(secs);
        self
    }

    pub fn max_message_retry_times(mut self, retry_times: i32) -> Self {
        self.config.data.max_message_retry_times = Some(retry_times);
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

    pub fn build(self) -> Engine {
        Engine::new()
            .with_config(&self.config)
            .set_plugins(self.plugins.clone())
    }
}
