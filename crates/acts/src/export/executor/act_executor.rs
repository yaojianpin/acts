use crate::event::EventAction;
use crate::{Action, Principal, Result, Vars, scheduler::Runtime};
use std::sync::Arc;

/// `act:submit` — submit an IRQ task's work for review.
pub(crate) const SUBMIT: &str = "act:submit";
/// `act:back` — send a submitted task back to its IRQ owner.
pub(crate) const BACK: &str = "act:back";
/// `act:cancel` — cancel a task.
pub(crate) const CANCEL: &str = "act:cancel";
/// `act:complete` — complete a task and move the process on.
pub(crate) const COMPLETE: &str = "act:complete";
/// `act:abort` — abort a task.
pub(crate) const ABORT: &str = "act:abort";
/// `act:skip` — skip a task.
pub(crate) const SKIP: &str = "act:skip";
/// `act:error` — fail a task.
pub(crate) const ERROR: &str = "act:error";
/// `act:push` — push a new branch task onto the tree.
pub(crate) const PUSH: &str = "act:push";
/// `act:remove` — remove a task from the tree.
pub(crate) const REMOVE: &str = "act:remove";
/// `act:set_vars` — write a task's scope vars without changing its state.
pub(crate) const SET_VARS: &str = "act:set_vars";

#[derive(Clone)]
pub struct ActExecutor {
    runtime: Arc<Runtime>,
    principal: Arc<Principal>,
}

impl ActExecutor {
    pub(crate) fn new(rt: &Arc<Runtime>, principal: &Arc<Principal>) -> Self {
        Self {
            runtime: rt.clone(),
            principal: principal.clone(),
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

    /// Run one task action. The action the caller names is the action the
    /// principal is checked against — the wrappers above are spellings of the
    /// same check, never a way around it.
    pub async fn do_action(
        &self,
        pid: &str,
        tid: &str,
        action: EventAction,
        options: Vars,
    ) -> Result<()> {
        self.principal.check(name_of(&action))?;
        self.runtime
            .do_action(&Action::new(pid, tid, action, options))
            .await
    }
}

/// The action name of a task action, as the shared dispatch table spells it.
fn name_of(action: &EventAction) -> &'static str {
    match action {
        EventAction::Next => COMPLETE,
        EventAction::Submit => SUBMIT,
        EventAction::Back => BACK,
        EventAction::Cancel => CANCEL,
        EventAction::Abort => ABORT,
        EventAction::Skip => SKIP,
        EventAction::Error => ERROR,
        EventAction::Push => PUSH,
        EventAction::Remove => REMOVE,
        EventAction::SetProcessVars => SET_VARS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every task action has a name, and the names are the ones the dispatch
    /// table grants — `act:complete` for `Next`, not `act:next`.
    #[test]
    fn every_task_action_has_its_wire_name() {
        for (action, name) in [
            (EventAction::Next, "act:complete"),
            (EventAction::Submit, "act:submit"),
            (EventAction::Back, "act:back"),
            (EventAction::Cancel, "act:cancel"),
            (EventAction::Abort, "act:abort"),
            (EventAction::Skip, "act:skip"),
            (EventAction::Error, "act:error"),
            (EventAction::Push, "act:push"),
            (EventAction::Remove, "act:remove"),
            (EventAction::SetProcessVars, "act:set_vars"),
        ] {
            assert_eq!(name_of(&action), name);
        }
    }
}
