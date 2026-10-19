mod action;
mod emitter;
mod message;

use crate::{ActError, Result};
pub use action::Action;
pub(crate) use emitter::ActWorkflowMessageHandle;
pub use emitter::Emitter;
pub(crate) use emitter::ProcessGate;
pub use message::{Message, MessageState};

use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Clone)]
pub struct Event<T> {
    inner: T,
}

#[derive(
    Serialize,
    Deserialize,
    Debug,
    Clone,
    Default,
    PartialEq,
    strum::AsRefStr,
    strum::EnumString,
    strum::EnumIter,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum EventAction {
    #[default]
    Next,
    Submit,
    Back,
    Cancel,
    Abort,
    Skip,
    Error,
    Push,
    Remove,
    SetProcessVars,
}

impl EventAction {
    pub fn parse(v: &str) -> Result<Self> {
        Self::from_str(v)
            .map_err(|_| ActError::Action(format!("cannot find the action define '{v}'")))
    }
}

impl<T> std::ops::Deref for Event<T>
where
    T: std::fmt::Debug + Clone,
{
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> Event<T>
where
    T: std::fmt::Debug + Clone,
{
    pub fn inner(&self) -> &T {
        &self.inner
    }

    pub fn from_inner(inner: T) -> Self {
        Self { inner }
    }
}

impl<T> std::fmt::Debug for Event<T>
where
    T: std::fmt::Debug + Clone,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("{:?}", self.inner))
    }
}

impl std::fmt::Display for EventAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_a_wire_name_and_reports_an_unknown_one() {
        assert_eq!(EventAction::parse("next").unwrap(), EventAction::Next);
        assert_eq!(
            EventAction::parse("set_process_vars").unwrap(),
            EventAction::SetProcessVars
        );
        let err = EventAction::parse("no_such_action").unwrap_err();
        assert!(
            err.to_string()
                .contains("cannot find the action define 'no_such_action'"),
            "got: {err}"
        );
    }

    #[test]
    fn display_matches_the_wire_name() {
        assert_eq!(EventAction::Next.to_string(), "next");
        assert_eq!(EventAction::Submit.to_string(), "submit");
        assert_eq!(EventAction::SetProcessVars.to_string(), "set_process_vars");
    }

    #[test]
    fn an_event_wraps_its_inner_value() {
        let event = Event::from_inner(7u8);
        assert_eq!(*event, 7);
        assert_eq!(event.inner(), &7);
        assert_eq!(format!("{event:?}"), "7");
    }
}
