use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Retry {
    /// times to retry
    /// 0 means no retry
    #[serde(default)]
    pub times: i32,
}

impl Retry {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn with_times(mut self, times: i32) -> Self {
        self.times = times;
        self
    }
}
