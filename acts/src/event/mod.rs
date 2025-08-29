mod action;
mod emitter;
mod extra;
mod message;

#[cfg(test)]
mod tests;

use crate::{ActError, Result, scheduler::Runtime};
pub use action::Action;
pub use emitter::Emitter;
pub use extra::TaskExtra;
pub use message::{Message, MessageState, Model};

use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::Arc;

#[derive(Clone)]
pub struct Event<T, E = ()> {
    inner: T,
    extra: E,
    #[cfg(test)]
    pub(crate) runtime: Option<Arc<Runtime>>,
}

#[derive(
    Serialize, Deserialize, Debug, Clone, Default, PartialEq, strum::AsRefStr, strum::EnumString,
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
    SetVars,
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
    pub fn new(_s: &Option<Arc<Runtime>>, inner: &T) -> Self {
        Self {
            #[cfg(test)]
            runtime: _s.clone(),
            extra: E::default(),
            inner: inner.clone(),
        }
    }

    pub fn new_with_extra(_rt: &Option<Arc<Runtime>>, inner: &T, extra: &E) -> Self {
        Self {
            #[cfg(test)]
            runtime: _rt.clone(),
            extra: extra.clone(),
            inner: inner.clone(),
        }
    }

    pub fn extra(&self) -> &E {
        &self.extra
    }

    #[cfg(test)]
    pub fn do_action(
        &self,
        pid: &str,
        tid: &str,
        action: EventAction,
        options: &crate::Vars,
    ) -> Result<()> {
        if let Some(scher) = &self.runtime {
            return scher.do_action(&Action::new(pid, tid, action, options));
        }
        Err(ActError::Action("scher is not define in Event".to_string()))
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
