#[derive(Debug, Clone)]
pub struct Config {
    pub cache_cap: usize,
    pub log_dir: String,
    pub log_level: String,
    pub data_dir: String,
    pub db_name: String,
    pub tick_interval_secs: u64,

    // will delete message after the max retries
    // cancel the settings by setting to 0
    pub max_message_retry_times: i32,
    // do not remove process and tasks on complete
    pub keep_processes: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cache_cap: 1024,
            log_dir: "log".to_string(),
            data_dir: "data".to_string(),
            db_name: "acts.db".to_string(),
            log_level: "INFO".to_string(),

            // default to 15s
            tick_interval_secs: 15,
            max_message_retry_times: 20,
            keep_processes: false,
        }
    }
}
