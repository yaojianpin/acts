//! cron expression helpers for `schedule` triggers.
//!
//! The cron syntax follows the `cron` crate: 6 fields
//! (`sec min hour day month dow`). Times are interpreted in the engine's
//! local timezone.

use crate::Result;
use chrono::{DateTime, Local};
use std::str::FromStr;

/// A parsed cron schedule.
#[derive(Debug, Clone)]
pub struct Cron(cron::Schedule);

impl Cron {
    /// parse a 6-field cron expression (`sec min hour day month dow`)
    pub fn parse(expr: &str) -> Result<Self> {
        let schedule = cron::Schedule::from_str(expr)
            .map_err(|err| crate::ActError::Model(format!("invalid cron '{expr}': {err}")))?;
        Ok(Self(schedule))
    }

    /// the next fire time strictly after `after`, in the same timezone
    pub fn next_after(&self, after: DateTime<Local>) -> Option<DateTime<Local>> {
        self.0.after(&after).next()
    }

    /// the next fire time after now
    pub fn next(&self) -> Option<DateTime<Local>> {
        self.next_after(Local::now())
    }

    /// the next fire time of `expr` after now, in epoch millis — 0 when the
    /// expression can't be parsed or has no next fire. Used to arm a
    /// `schedule` trigger row on deploy and to roll its `next_run` forward
    /// after firing.
    pub fn next_fire_millis(expr: &str) -> i64 {
        Self::parse(expr)
            .ok()
            .and_then(|cron| cron.next())
            .map(|next| next.timestamp_millis())
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cron_parse_ok() {
        assert!(Cron::parse("* * * * * *").is_ok());
        assert!(Cron::parse("0 0 12 * * mon-fri").is_ok());
    }

    #[test]
    fn cron_parse_err() {
        assert!(Cron::parse("not a cron").is_err());
    }

    #[test]
    fn cron_next_is_after_now() {
        let c = Cron::parse("* * * * * *").unwrap();
        let next = c.next().unwrap();
        assert!(next > Local::now());
    }
}
