mod action;
mod emitter;
mod extra;
mod message;

#[cfg(test)]
mod tests;

use crate::{ActError, Result};
pub use action::Action;
pub use emitter::Emitter;
pub use extra::TaskExtra;
pub use message::{Message, MessageState};

use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Clone)]
pub struct Event<T, E = ()> {
    inner: T,
    extra: E,
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

impl<T, E> std::ops::Deref for Event<T, E>
where
    T: std::fmt::Debug + Clone,
    E: std::fmt::Debug + Clone + Default,
{
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T, E> Event<T, E>
where
    T: std::fmt::Debug + Clone,
    E: std::fmt::Debug + Clone + Default,
{
    pub fn inner(&self) -> &T {
        &self.inner
    }
    pub fn new(inner: &T) -> Self {
        Self {
            extra: E::default(),
            inner: inner.clone(),
        }
    }

    pub fn new_with_extra(inner: &T, extra: &E) -> Self {
        Self {
            extra: extra.clone(),
            inner: inner.clone(),
        }
    }

    pub fn extra(&self) -> &E {
        &self.extra
    }
}

impl<T, E> std::fmt::Debug for Event<T, E>
where
    T: std::fmt::Debug + Clone,
    E: std::fmt::Debug + Clone + Default,
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
