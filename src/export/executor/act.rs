use crate::event::EventAction;
use crate::{sch::Runtime, Action, Result, Vars};
use std::sync::Arc;

#[derive(Clone)]
pub struct ActExecutor {
    runtime: Arc<Runtime>,
}

impl ActExecutor {
    pub fn new(rt: &Arc<Runtime>) -> Self {
        Self {
            runtime: rt.clone(),
        }
    }

    pub fn submit(&self, pid: &str, tid: &str, options: &Vars) -> Result<()> {
        self.do_action(pid, EventAction::Submit, tid, options)
    }

    pub fn back(&self, pid: &str, tid: &str, options: &Vars) -> Result<()> {
        self.do_action(pid, EventAction::Back, tid, options)
    }

    pub fn cancel(&self, pid: &str, tid: &str, options: &Vars) -> Result<()> {
        self.do_action(pid, EventAction::Cancel, tid, options)
    }

    pub fn complete(&self, pid: &str, tid: &str, options: &Vars) -> Result<()> {
        self.do_action(pid, EventAction::Next, tid, options)
    }

    pub fn abort(&self, pid: &str, tid: &str, options: &Vars) -> Result<()> {
        self.do_action(pid, EventAction::Abort, tid, options)
    }

    pub fn skip(&self, pid: &str, tid: &str, options: &Vars) -> Result<()> {
        self.do_action(pid, EventAction::Skip, tid, options)
    }

    pub fn error(&self, pid: &str, tid: &str, options: &Vars) -> Result<()> {
        self.do_action(pid, EventAction::Error, tid, options)
    }

    pub fn push(&self, pid: &str, tid: &str, options: &Vars) -> Result<()> {
        self.do_action(pid, EventAction::Push, tid, options)
    }

    pub fn remove(&self, pid: &str, tid: &str, options: &Vars) -> Result<()> {
        self.do_action(pid, EventAction::Remove, tid, options)
    }

    fn do_action(&self, pid: &str, action: EventAction, tid: &str, options: &Vars) -> Result<()> {
        self.runtime
            .do_action(&Action::new(pid, tid, action, options))
    }
}
