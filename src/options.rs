#[derive(Debug, Clone)]
pub struct Options {
    pub cache_cap: usize,
    pub log_dir: String,
    pub log_level: String,
    pub data_dir: String,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            cache_cap: 100,
            log_dir: "log".to_string(),
            data_dir: "data".to_string(),
            log_level: "INFO".to_string(),
        }
    }
}
