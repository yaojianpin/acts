use std::time::{SystemTime, UNIX_EPOCH};

/// Time source for orchestration code. Domain state machines receive explicit
/// timestamps and remain deterministic.
pub(crate) trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SystemClock;
impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => i64::try_from(duration.as_millis()).unwrap_or(i64::MAX),
            Err(error) => -i64::try_from(error.duration().as_millis()).unwrap_or(i64::MAX),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn system_clock_produces_a_representable_timestamp() {
        let now = SystemClock.now_ms();
        assert!(now > 0);
    }
}
