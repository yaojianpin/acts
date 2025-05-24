use hocon::Hocon;
use serde::Deserialize;
use std::{ops::Deref, path::Path};

#[derive(Debug, Clone)]
pub struct Config {
    pub data: ConfigData,
    pub table: Hocon,
}

#[derive(Debug, Clone)]
pub struct ConfigData {
    pub cache_cap: i64,
    pub log_dir: String,
    pub log_level: String,
    // pub db_name: String,
    pub tick_interval_secs: i64,

    // will delete message after the max retries
    // cancel the settings by setting to 0
    pub max_message_retry_times: i32,
    // do not remove process and tasks on complete
    pub keep_processes: bool,
}

impl Default for ConfigData {
    fn default() -> Self {
        Self {
            cache_cap: 1024,
            log_dir: "log".to_string(),
            log_level: "INFO".to_string(),
            tick_interval_secs: 15,
            max_message_retry_times: 20,
            keep_processes: false,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            data: ConfigData::default(),
            table: Hocon::Null,
        }
    }
}

impl From<Hocon> for ConfigData {
    fn from(value: Hocon) -> Self {
        Self {
            cache_cap: value["cache_cap"].as_i64().unwrap_or(1024),
            log_dir: value["log_dir"].as_string().unwrap_or("log".to_string()),
            log_level: value["log_level"].as_string().unwrap_or("INFO".to_string()),
            tick_interval_secs: value["tick_interval_secs"].as_i64().unwrap_or(15),
            max_message_retry_times: value["max_message_retry_times"].as_i64().unwrap_or(20) as i32,
            keep_processes: value["keep_processes"].as_bool().unwrap_or_default(),
        }
    }
}

impl Deref for Config {
    type Target = ConfigData;
    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl Config {
    pub fn create(path: &Path) -> Self {
        #[allow(clippy::expect_fun_call)]
        let table = hocon::HoconLoader::new()
            .load_file(path)
            .expect(&format!("failed to load config file: {:?}", path))
            .hocon()
            .expect(&format!("failed to parse config file: {:?}", path));

        Self {
            table: table.clone(),
            data: table.into(),
        }
    }

    pub fn get<'de, T>(&self, name: &str) -> crate::Result<T>
    where
        T: Deserialize<'de>,
    {
        let value = self.table[name].clone();
        value.resolve().map_err(|err| {
            crate::ActError::Config(format!("failed to get '{}' config: {}", name, err))
        })
    }
}
