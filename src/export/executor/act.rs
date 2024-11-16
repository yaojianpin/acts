use crate::{sch::Runtime, utils::consts, Action, Result, Vars};
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
        self.do_action(pid, consts::EVT_SUBMIT, tid, options)
    }

    pub fn back(&self, pid: &str, tid: &str, options: &Vars) -> Result<()> {
        self.do_action(pid, consts::EVT_BACK, tid, options)
    }

    pub fn cancel(&self, pid: &str, tid: &str, options: &Vars) -> Result<()> {
        self.do_action(pid, consts::EVT_CANCEL, tid, options)
    }

    pub fn complete(&self, pid: &str, tid: &str, options: &Vars) -> Result<()> {
        self.do_action(pid, consts::EVT_NEXT, tid, options)
    }

    pub fn abort(&self, pid: &str, tid: &str, options: &Vars) -> Result<()> {
        self.do_action(pid, consts::EVT_ABORT, tid, options)
    }

    pub fn skip(&self, pid: &str, tid: &str, options: &Vars) -> Result<()> {
        self.do_action(pid, consts::EVT_SKIP, tid, options)
    }

    pub fn error(&self, pid: &str, tid: &str, options: &Vars) -> Result<()> {
        self.do_action(pid, consts::EVT_ERR, tid, options)
    }

    pub fn push(&self, pid: &str, tid: &str, options: &Vars) -> Result<()> {
        self.do_action(pid, consts::EVT_PUSH, tid, options)
    }

    pub fn remove(&self, pid: &str, tid: &str, options: &Vars) -> Result<()> {
        self.do_action(pid, consts::EVT_REMOVE, tid, options)
    }

    fn do_action(&self, pid: &str, action: &str, tid: &str, options: &Vars) -> Result<()> {
        self.runtime
            .do_action(&Action::new(pid, tid, action, options))
    }
}
