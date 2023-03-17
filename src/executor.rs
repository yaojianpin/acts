use crate::{
    debug,
    sch::{ActionOptions, Scheduler},
    ActResult, UserMessage, Workflow,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct Executor {
    scher: Arc<Scheduler>,
}

impl Executor {
    pub(crate) fn new(sch: &Arc<Scheduler>) -> Self {
        Self { scher: sch.clone() }
    }

    pub fn start(&self, workflow: &Workflow) {
        self.scher.start(workflow);
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

    pub fn complete(&self, pid: &str, uid: &str, options: Option<ActionOptions>) -> ActResult<()> {
        self.do_action(pid, "complete", uid, options)
    }

    pub fn abort(&self, pid: &str, uid: &str, options: Option<ActionOptions>) -> ActResult<()> {
        self.do_action(pid, "abort", uid, options)
    }

    pub fn delete(&self, pid: &str) -> ActResult<bool> {
        self.scher.cache().remove(pid)
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
