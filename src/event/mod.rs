mod action;
mod emitter;
mod extra;
mod message;

#[cfg(test)]
mod tests;

use std::fmt;

use crate::{
    sch::Scheduler,
    utils::{self, consts},
    ActError, Result,
};
pub use action::Action;
pub use emitter::Emitter;
pub use extra::TaskExtra;
pub use message::{Message, Model};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone)]
pub struct Event<T, E = ()> {
    inner: T,
    extra: E,
    scher: Option<Arc<Scheduler>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub enum EventAction {
    #[default]
    Complete,
    Submit,
    Back,
    Cancel,
    Abort,
    Skip,
    Error,
    Push,
    Remove,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub enum ActionState {
    #[default]
    None,
    Created,
    Completed,
    Submitted,
    Backed,
    Cancelled,
    Aborted,
    Skipped,
    Error,
    Removed,
}

impl EventAction {
    pub fn parse(v: &str) -> Result<Self> {
        match v {
            consts::EVT_BACK => Ok(EventAction::Back),
            consts::EVT_CANCEL => Ok(EventAction::Cancel),
            consts::EVT_ABORT => Ok(EventAction::Abort),
            consts::EVT_SUBMIT => Ok(EventAction::Submit),
            consts::EVT_SKIP => Ok(EventAction::Skip),
            consts::EVT_COMPLETE => Ok(EventAction::Complete),
            consts::EVT_ERR => Ok(EventAction::Error),
            consts::EVT_PUSH => Ok(EventAction::Push),
            consts::EVT_REMOVE => Ok(EventAction::Remove),
            _ => Err(ActError::Action(format!(
                "cannot find the action define '{v}'"
            ))),
        }
    }
    pub fn state(&self) -> ActionState {
        match self {
            EventAction::Complete => ActionState::Completed,
            EventAction::Submit => ActionState::Submitted,
            EventAction::Back => ActionState::Backed,
            EventAction::Cancel => ActionState::Cancelled,
            EventAction::Abort => ActionState::Aborted,
            EventAction::Skip => ActionState::Skipped,
            EventAction::Error => ActionState::Error,
            EventAction::Remove => ActionState::Removed,
            _ => ActionState::None,
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
    pub fn new(s: &Option<Arc<Scheduler>>, inner: &T) -> Self {
        Self {
            scher: s.clone(),
            extra: E::default(),
            inner: inner.clone(),
        }
    }

    pub fn new_with_extra(s: &Option<Arc<Scheduler>>, inner: &T, extra: &E) -> Self {
        Self {
            scher: s.clone(),
            extra: extra.clone(),
            inner: inner.clone(),
        }
    }

    pub fn extra(&self) -> &E {
        &self.extra
    }

    pub fn close(&self) {
        if let Some(s) = &self.scher {
            s.close();
        }
    }

    #[cfg(test)]
    pub fn do_action(
        &self,
        pid: &str,
        tid: &str,
        action: &str,
        options: &crate::Vars,
    ) -> Result<crate::ActionResult> {
        if let Some(scher) = &self.scher {
            return scher.do_action(&Action::new(pid, tid, action, options));
        }
        Err(ActError::Action(format!("scher is not define in Event")))
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
            EventAction::Complete => f.write_str(consts::EVT_COMPLETE),
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

impl fmt::Display for ActionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::result::Result<(), std::fmt::Error> {
        let s: String = self.into();
        f.write_str(&s)
    }
}

impl From<ActionState> for String {
    fn from(state: ActionState) -> Self {
        utils::action_state_to_str(state)
    }
}

impl From<&str> for ActionState {
    fn from(str: &str) -> Self {
        utils::str_to_action_state(str)
    }
}

impl From<String> for ActionState {
    fn from(str: String) -> Self {
        utils::str_to_action_state(&str)
    }
}

impl From<&ActionState> for String {
    fn from(state: &ActionState) -> Self {
        utils::action_state_to_str(state.clone())
    }
}
