use crate::event::EventAction;
use crate::{Action, Result, Vars, scheduler::Runtime};
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
        self.do_action(pid, tid, EventAction::Submit, options)
    }

    pub fn back(&self, pid: &str, tid: &str, options: &Vars) -> Result<()> {
        self.do_action(pid, tid, EventAction::Back, options)
    }

    pub fn cancel(&self, pid: &str, tid: &str, options: &Vars) -> Result<()> {
        self.do_action(pid, tid, EventAction::Cancel, options)
    }

    pub fn complete(&self, pid: &str, tid: &str, options: &Vars) -> Result<()> {
        self.do_action(pid, tid, EventAction::Next, options)
    }

    pub fn abort(&self, pid: &str, tid: &str, options: &Vars) -> Result<()> {
        self.do_action(pid, tid, EventAction::Abort, options)
    }

    pub fn skip(&self, pid: &str, tid: &str, options: &Vars) -> Result<()> {
        self.do_action(pid, tid, EventAction::Skip, options)
    }

    pub fn error(&self, pid: &str, tid: &str, options: &Vars) -> Result<()> {
        self.do_action(pid, tid, EventAction::Error, options)
    }

    pub fn push(&self, pid: &str, tid: &str, options: &Vars) -> Result<()> {
        self.do_action(pid, tid, EventAction::Push, options)
    }

    pub fn remove(&self, pid: &str, tid: &str, options: &Vars) -> Result<()> {
        self.do_action(pid, tid, EventAction::Remove, options)
    }

    pub fn set_task_vars(&self, pid: &str, tid: &str, options: &Vars) -> Result<()> {
        self.do_action(pid, tid, EventAction::SetVars, options)
    }

    pub fn set_process_vars(&self, pid: &str, tid: &str, options: &Vars) -> Result<()> {
        self.do_action(pid, tid, EventAction::SetProcessVars, options)
    }

    pub fn do_action(
        &self,
        pid: &str,
        tid: &str,
        action: EventAction,
        options: &Vars,
    ) -> Result<()> {
        self.runtime
            .do_action(&Action::new(pid, tid, action, options))
    }
}
