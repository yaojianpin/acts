use crate::{
    event::Action,
    sch::Scheduler,
    store::{Store, StoreAdapter},
    utils::consts,
    ActError, ActResult, ActionState, ModelInfo, Vars,
};
use std::sync::Arc;
use tracing::info;

#[derive(Clone)]
pub struct Executor {
    scher: Arc<Scheduler>,
    store: Arc<Store>,
}

impl Executor {
    pub(crate) fn new(sch: &Arc<Scheduler>, store: &Arc<Store>) -> Self {
        Self {
            scher: sch.clone(),
            store: store.clone(),
        }
    }

    pub fn start(&self, mid: &str, options: &Vars) -> ActResult<ActionState> {
        let model: ModelInfo = self.store.models().find(mid)?.into();
        let workflow = model.workflow()?;

        let mut vars = options.clone();
        // set the workflow initiator
        if let Some(uid) = options.get("uid") {
            vars.insert(consts::INITIATOR.to_string(), uid.clone());
        }

        self.scher.start(&workflow, &vars)
    }

    pub fn submit(&self, mid: &str, options: &Vars) -> ActResult<ActionState> {
        let uid = options
            .get(consts::UID)
            .ok_or(ActError::Action(format!("cannot find uid in options")))?;
        let model: ModelInfo = self.store.models().find(mid)?.into();
        let workflow = model.workflow()?;

        let mut vars = options.clone();
        vars.insert(consts::AUTO_SUBMIT.to_string(), true.into());
        vars.insert(consts::INITIATOR.to_string(), uid.clone());

        self.scher.start(&workflow, &vars)
    }

    pub fn back(&self, pid: &str, aid: &str, options: &Vars) -> ActResult<ActionState> {
        self.do_action(pid, consts::EVT_BACK, aid, options)
    }

    pub fn cancel(&self, pid: &str, aid: &str, options: &Vars) -> ActResult<ActionState> {
        self.do_action(pid, consts::EVT_CANCEL, aid, options)
    }

    pub fn complete(&self, pid: &str, aid: &str, options: &Vars) -> ActResult<ActionState> {
        self.do_action(pid, consts::EVT_COMPLETE, aid, options)
    }

    pub fn abort(&self, pid: &str, aid: &str, options: &Vars) -> ActResult<ActionState> {
        self.do_action(pid, consts::EVT_ABORT, aid, options)
    }

    pub fn update(&self, pid: &str, aid: &str, options: &Vars) -> ActResult<ActionState> {
        self.do_action(pid, consts::EVT_UPDATE, aid, options)
    }

    pub fn ack(&self, pid: &str, aid: &str) -> ActResult<ActionState> {
        info!("ack pid={} aid={}", pid, aid);
        self.scher.do_ack(pid, aid)
    }

    fn do_action(
        &self,
        pid: &str,
        action: &str,
        aid: &str,
        options: &Vars,
    ) -> ActResult<ActionState> {
        info!(
            "do_action action={} pid={}, aid={} options={:?}",
            action, pid, aid, options
        );

        let act = Action::new(pid, aid, action, options);
        self.scher.do_action(&act)
    }
}
