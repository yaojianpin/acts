use std::sync::Arc;

use crate::{
    event::Action, sch::Scheduler, store::StoreAdapter, utils::consts, ActionResult, ModelInfo,
    Result, Vars,
};

#[derive(Clone)]
pub struct Executor {
    scher: Arc<Scheduler>,
}

impl Executor {
    pub(crate) fn new(scher: &Arc<Scheduler>) -> Self {
        Self {
            scher: scher.clone(),
        }
    }

    pub fn start(&self, mid: &str, options: &Vars) -> Result<ActionResult> {
        let mut state = ActionResult::begin();
        let model: ModelInfo = self.scher.cache().store().models().find(mid)?.into();
        let workflow = model.workflow()?;

        let mut vars = options.clone();
        // set the workflow initiator
        if let Some(uid) = options.get_value(consts::FOR_ACT_KEY_UID) {
            vars.insert(consts::INITIATOR.to_string(), uid.clone());
        }
        let proc = self.scher.start(&workflow, &vars)?;
        state.insert("pid", proc.id().into());
        state.end();

        Ok(state)
    }

    pub fn submit(&self, pid: &str, tid: &str, options: &Vars) -> Result<ActionResult> {
        self.do_action(pid, consts::EVT_SUBMIT, tid, options)
    }

    pub fn back(&self, pid: &str, tid: &str, options: &Vars) -> Result<ActionResult> {
        self.do_action(pid, consts::EVT_BACK, tid, options)
    }

    pub fn cancel(&self, pid: &str, tid: &str, options: &Vars) -> Result<ActionResult> {
        self.do_action(pid, consts::EVT_CANCEL, tid, options)
    }

    pub fn complete(&self, pid: &str, tid: &str, options: &Vars) -> Result<ActionResult> {
        self.do_action(pid, consts::EVT_COMPLETE, tid, options)
    }

    pub fn abort(&self, pid: &str, tid: &str, options: &Vars) -> Result<ActionResult> {
        self.do_action(pid, consts::EVT_ABORT, tid, options)
    }

    pub fn skip(&self, pid: &str, tid: &str, options: &Vars) -> Result<ActionResult> {
        self.do_action(pid, consts::EVT_SKIP, tid, options)
    }

    pub fn error(&self, pid: &str, tid: &str, options: &Vars) -> Result<ActionResult> {
        self.do_action(pid, consts::EVT_ERR, tid, options)
    }

    pub fn push(&self, pid: &str, tid: &str, options: &Vars) -> Result<ActionResult> {
        self.do_action(pid, consts::EVT_PUSH, tid, options)
    }

    pub fn remove(&self, pid: &str, tid: &str, options: &Vars) -> Result<ActionResult> {
        self.do_action(pid, consts::EVT_REMOVE, tid, options)
    }

    fn do_action(
        &self,
        pid: &str,
        action: &str,
        tid: &str,
        options: &Vars,
    ) -> Result<ActionResult> {
        self.scher
            .do_action(&Action::new(pid, tid, action, options))
    }
}
