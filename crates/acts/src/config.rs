use serde::Deserialize;
use std::path::Path;
use toml::Table;

#[derive(Debug, Clone)]
pub struct Config {
    pub data: ConfigData,
    pub table: Table,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConfigLog {
    pub dir: String,
    pub level: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ConfigData {
    pub cache_cap: Option<i64>,
    pub tick_interval_secs: Option<i64>,

    // will delete message after the max retries
    // cancel the settings by setting to 0
    pub max_message_retry_times: Option<i32>,
    // do not remove process and tasks on complete
    pub keep_processes: Option<bool>,

    // log config
    pub log: Option<ConfigLog>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            data: ConfigData::default(),
            table: Table::new(),
        }
    }
}

impl Config {
    pub fn create(path: &Path) -> Self {
        #[allow(clippy::expect_fun_call)]
        let data =
            std::fs::read_to_string(path).expect(&format!("failed to load config file {:?}", path));

        #[allow(clippy::expect_fun_call)]
        let table = toml::from_str::<Table>(data.as_str())
            .expect(&format!("failed to parse the toml file({:?})", path));

        let data = ConfigData::deserialize(table.clone()).unwrap();
        Self {
            table: table.clone(),
            data,
        }
    }

    pub fn get<'de, T>(&self, name: &str) -> crate::Result<T>
    where
        T: Deserialize<'de>,
    {
        let value = self.table[name].clone();
        T::deserialize(value).map_err(|err| {
            crate::ActError::Config(format!("failed to get '{}' config: {}", name, err))
        })
    }

    pub fn has(&self, name: &str) -> bool {
        self.table.contains_key(name)
    }

    pub fn cache_cap(&self) -> i64 {
        self.data.cache_cap.unwrap_or(1024)
    }
    pub fn keep_processes(&self) -> bool {
        self.data.keep_processes.unwrap_or(false)
    }
    pub fn max_message_retry_times(&self) -> i32 {
        self.data.max_message_retry_times.unwrap_or(20)
    }
    pub fn tick_interval_secs(&self) -> i64 {
        self.data.tick_interval_secs.unwrap_or(15)
    }

    pub fn log(&self) -> ConfigLog {
        self.data.log.clone().unwrap_or(ConfigLog {
            dir: "log".to_string(),
            level: "INFO".to_string(),
        })
    }
}
