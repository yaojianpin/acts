#[derive(Debug, Clone)]
pub struct Config {
    pub cache_cap: usize,
    pub log_dir: String,
    pub log_level: String,
    pub data_dir: String,
    pub db_name: String,
    pub tick_interval_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cache_cap: 256,
            log_dir: "log".to_string(),
            data_dir: "data".to_string(),
            db_name: "acts.db".to_string(),
            log_level: "INFO".to_string(),

            // default to 15s
            tick_interval_secs: 15,
        }
    }
}
