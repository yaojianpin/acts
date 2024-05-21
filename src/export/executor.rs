use std::sync::Arc;

use crate::{
    event::Action, sch::Runtime, store::StoreAdapter, utils::consts, ModelInfo, Result, Vars,
};

#[derive(Clone)]
pub struct Executor {
    runtime: Arc<Runtime>,
}

impl Executor {
    pub(crate) fn new(rt: &Arc<Runtime>) -> Self {
        Self {
            runtime: rt.clone(),
        }
    }

    pub fn start(&self, mid: &str, options: &Vars) -> Result<String> {
        let model: ModelInfo = self.runtime.cache().store().models().find(mid)?.into();
        let workflow = model.workflow()?;

        let mut vars = options.clone();
        // set the workflow initiator
        if let Some(uid) = options.get_value(consts::FOR_ACT_KEY_UID) {
            vars.insert(consts::INITIATOR.to_string(), uid.clone());
        }
        let proc = self.runtime.start(&workflow, &vars)?;
        Ok(proc.id().to_string())
    }

    pub fn ack(&self, id: &str) -> Result<()> {
        self.runtime.ack(id)
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
