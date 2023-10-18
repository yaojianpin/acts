use crate::{
    event::Action, sch::Scheduler, store::StoreAdapter, utils::consts, ActionResult, ModelInfo,
    Result, Vars,
};
use std::sync::Arc;
use tracing::error;

#[derive(Clone)]
pub struct Executor {
    scher: Arc<Scheduler>,
}

impl Executor {
    pub(crate) fn new(sch: &Arc<Scheduler>) -> Self {
        Self { scher: sch.clone() }
    }

    pub fn start(&self, mid: &str, options: &Vars) -> Result<ActionResult> {
        let model: ModelInfo = self.scher.cache().store().models().find(mid)?.into();
        let workflow = model.workflow()?;

        let mut vars = options.clone();
        // set the workflow initiator
        if let Some(uid) = options.get(consts::FOR_ACT_KEY_UID) {
            vars.insert(consts::INITIATOR.to_string(), uid.clone());
        }

        self.scher.start(&workflow, &vars)
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

    fn do_action(
        &self,
        pid: &str,
        action: &str,
        tid: &str,
        options: &Vars,
    ) -> Result<ActionResult> {
        let act = Action::new(pid, tid, action, options);
        let ret = self.scher.do_action(&act);
        if let Err(err) = ret.as_ref() {
            error!("{}", err);
        }

        ret
    }
}
