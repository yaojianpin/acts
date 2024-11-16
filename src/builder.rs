use std::sync::Arc;

use crate::{Config, Engine, StoreAdapter};

pub struct Builder {
    config: Config,
    store: Option<Arc<dyn StoreAdapter>>,
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

impl Builder {
    pub fn new() -> Self {
        Self {
            config: Config::default(),
            store: None,
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

    pub fn data_dir(mut self, data_dir: &str) -> Self {
        self.config.data_dir = data_dir.to_string();
        self
    }

    pub fn db_name(mut self, db_name: &str) -> Self {
        self.config.db_name = db_name.to_string();
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

    pub fn store<STORE: StoreAdapter + Clone + 'static>(mut self, store: &STORE) -> Self {
        self.store = Some(Arc::new(store.clone()));
        self
    }

    pub fn build(&self) -> Engine {
        Engine::new_with_config(&self.config, self.store.clone())
    }
}
