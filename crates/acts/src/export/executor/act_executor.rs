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

    pub async fn submit(&self, pid: &str, tid: &str, options: Vars) -> Result<()> {
        self.do_action(pid, tid, EventAction::Submit, options).await
    }

    pub async fn back(&self, pid: &str, tid: &str, options: Vars) -> Result<()> {
        self.do_action(pid, tid, EventAction::Back, options).await
    }

    pub async fn cancel(&self, pid: &str, tid: &str, options: Vars) -> Result<()> {
        self.do_action(pid, tid, EventAction::Cancel, options).await
    }

    pub async fn complete(&self, pid: &str, tid: &str, options: Vars) -> Result<()> {
        self.do_action(pid, tid, EventAction::Next, options).await
    }

    pub async fn abort(&self, pid: &str, tid: &str, options: Vars) -> Result<()> {
        self.do_action(pid, tid, EventAction::Abort, options).await
    }

    pub async fn skip(&self, pid: &str, tid: &str, options: Vars) -> Result<()> {
        self.do_action(pid, tid, EventAction::Skip, options).await
    }

    pub async fn fail(&self, pid: &str, tid: &str, options: Vars) -> Result<()> {
        self.do_action(pid, tid, EventAction::Error, options).await
    }

    pub async fn push(&self, pid: &str, tid: &str, options: Vars) -> Result<()> {
        self.do_action(pid, tid, EventAction::Push, options).await
    }

    pub async fn remove(&self, pid: &str, tid: &str, options: Vars) -> Result<()> {
        self.do_action(pid, tid, EventAction::Remove, options).await
    }

    pub async fn set_process_vars(&self, pid: &str, tid: &str, options: Vars) -> Result<()> {
        self.do_action(pid, tid, EventAction::SetProcessVars, options)
            .await
    }

    pub async fn do_action(
        &self,
        pid: &str,
        tid: &str,
        action: EventAction,
        options: Vars,
    ) -> Result<()> {
        self.runtime
            .do_action(&Action::new(pid, tid, action, options))
            .await
    }
}
