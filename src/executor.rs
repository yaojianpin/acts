use crate::{
    debug,
    sch::{ActionOptions, Scheduler},
    store::Store,
    ActResult, ModelInfo, UserMessage, Workflow,
};
use std::sync::Arc;

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

    pub fn deploy(&self, workflow: &Workflow) -> ActResult<bool> {
        self.store.deploy(&workflow)
    }

    pub fn start(&self, id: &str, options: ActionOptions) -> ActResult<bool> {
        let model: ModelInfo = self.store.model(id)?.into();
        let w = model.workflow()?;
        self.scher.start(&w, options)
    }

    pub fn submit(&self, pid: &str, uid: &str, options: Option<ActionOptions>) -> ActResult<()> {
        self.do_action(pid, "submit", uid, options)
    }

    pub fn back(&self, pid: &str, uid: &str, options: Option<ActionOptions>) -> ActResult<()> {
        self.do_action(pid, "back", uid, options)
    }

    pub fn cancel(&self, pid: &str, uid: &str, options: Option<ActionOptions>) -> ActResult<()> {
        self.do_action(pid, "cancel", uid, options)
    }

    pub fn next(&self, pid: &str, uid: &str, options: Option<ActionOptions>) -> ActResult<()> {
        self.do_action(pid, "next", uid, options)
    }

    pub fn abort(&self, pid: &str, uid: &str, options: Option<ActionOptions>) -> ActResult<()> {
        self.do_action(pid, "abort", uid, options)
    }

    fn do_action(
        &self,
        pid: &str,
        action: &str,
        uid: &str,
        options: Option<ActionOptions>,
    ) -> ActResult<()> {
        debug!(
            "do_action:{} action={} uid={} options={:?}",
            pid, action, uid, options
        );

        let message = UserMessage::new(pid, uid, action, options);
        self.scher.sched_message(&message);

        Ok(())
    }
}
