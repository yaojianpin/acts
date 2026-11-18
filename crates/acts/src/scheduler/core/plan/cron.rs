//! cron expression helpers for `schedule` triggers.
//!
//! Expressions follow `croner`: the engine documents the 6-field
//! (`sec min hour day month dow`) form, and a 5-field expression means
//! minute granularity. Times are interpreted in the engine's local timezone.

use crate::Result;
use croner::Cron as Croner;
use jiff::Zoned;
use std::str::FromStr;

/// A parsed cron schedule.
#[derive(Debug, Clone)]
pub struct Cron(Croner);

impl Cron {
    /// parse a cron expression (`sec min hour day month dow`)
    pub fn parse(expr: &str) -> Result<Self> {
        let schedule = Croner::from_str(expr)
            .map_err(|err| crate::ActError::Model(format!("invalid cron '{expr}': {err}")))?;
        Ok(Self(schedule))
    }

    /// the next fire time strictly after `after`, in `after`'s timezone
    fn next_after(&self, after: &Zoned) -> Option<Zoned> {
        self.0.find_next_occurrence(after, false).ok()
    }

    /// the next fire time after now
    fn next(&self) -> Option<Zoned> {
        self.next_after(&Zoned::now())
    }

    /// the next fire time of `expr` after now, in epoch millis — 0 when the
    /// expression can't be parsed or has no next fire. Used to arm a
    /// `schedule` trigger row on deploy and to roll its `next_run` forward
    /// after firing.
    pub fn next_fire_millis(expr: &str) -> i64 {
        Self::parse(expr)
            .ok()
            .and_then(|cron| cron.next())
            .map(|next| next.timestamp().as_millisecond())
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
        assert!(next > Zoned::now());
    }
}
