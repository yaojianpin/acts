#[derive(Debug, Clone)]
pub struct Options {
    pub cache_cap: usize,
    pub log_dir: String,
    pub log_level: String,
    pub data_dir: String,
    pub tick_interval_secs: u64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            cache_cap: 256,
            log_dir: "log".to_string(),
            data_dir: "data".to_string(),
            log_level: "INFO".to_string(),

            // default to 15s
            tick_interval_secs: 15,
        }
    }
}
