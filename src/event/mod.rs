mod action;
mod emitter;
mod extra;
mod message;

#[cfg(test)]
mod tests;

use crate::{sch::Runtime, utils::consts, ActError, Result};
pub use action::Action;
pub use emitter::Emitter;
pub use extra::TaskExtra;
pub use message::{Message, MessageState, Model};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone)]
pub struct Event<T, E = ()> {
    inner: T,
    extra: E,
    #[cfg(test)]
    pub(crate) runtime: Option<Arc<Runtime>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
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
}

impl EventAction {
    pub fn parse(v: &str) -> Result<Self> {
        match v {
            consts::EVT_BACK => Ok(EventAction::Back),
            consts::EVT_CANCEL => Ok(EventAction::Cancel),
            consts::EVT_ABORT => Ok(EventAction::Abort),
            consts::EVT_SUBMIT => Ok(EventAction::Submit),
            consts::EVT_SKIP => Ok(EventAction::Skip),
            consts::EVT_NEXT => Ok(EventAction::Next),
            consts::EVT_ERR => Ok(EventAction::Error),
            consts::EVT_PUSH => Ok(EventAction::Push),
            consts::EVT_REMOVE => Ok(EventAction::Remove),
            _ => Err(ActError::Action(format!(
                "cannot find the action define '{v}'"
            ))),
        }
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
        action: &str,
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
        match self {
            EventAction::Next => f.write_str(consts::EVT_NEXT),
            EventAction::Back => f.write_str(consts::EVT_BACK),
            EventAction::Cancel => f.write_str(consts::EVT_CANCEL),
            EventAction::Submit => f.write_str(consts::EVT_SUBMIT),
            EventAction::Abort => f.write_str(consts::EVT_ABORT),
            EventAction::Skip => f.write_str(consts::EVT_SKIP),
            EventAction::Error => f.write_str(consts::EVT_ERR),
            EventAction::Push => f.write_str(consts::EVT_PUSH),
            EventAction::Remove => f.write_str(consts::EVT_REMOVE),
        }
    }
}
