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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_builder_defaults_and_times() {
        let retry = Retry::new();
        assert_eq!(retry.times, 0);
        assert_eq!(retry.with_times(3).times, 3);
    }
}
