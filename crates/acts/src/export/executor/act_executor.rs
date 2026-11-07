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
/// `act:transfer` — move an assigned act's owner to another user.
pub(crate) const TRANSFER: &str = "act:transfer";
/// `act:delegate` — let another user decide an assigned act for its owner.
pub(crate) const DELEGATE: &str = "act:delegate";
/// `act:undelegate` — revoke an assigned act's delegation.
pub(crate) const UNDELEGATE: &str = "act:undelegate";
/// `act:remind` — a non-owner urges an assigned act.
pub(crate) const REMIND: &str = "act:remind";

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

    /// `act:transfer {pid,tid,to,expect_rev,reason}` — move the act's owner
    /// to another business user without deciding it. The owner-or-`assign`
    /// authorization runs in the kernel on the task's state.
    pub async fn transfer(&self, pid: &str, tid: &str, options: Vars) -> Result<()> {
        let to = options
            .get::<String>("to")
            .unwrap_or_default()
            .trim()
            .to_string();
        if to.is_empty() {
            return Err(crate::ActError::Action(
                "cannot find 'to' in act:transfer".to_string(),
            ));
        }
        self.do_action(pid, tid, EventAction::Transfer, options)
            .await
    }

    /// `act:delegate {pid,tid,delegatee,expires_at,reason}` — let another
    /// business user decide the act for its owner. The owner-or-`assign`
    /// authorization runs in the kernel on the task's state.
    pub async fn delegate(&self, pid: &str, tid: &str, options: Vars) -> Result<()> {
        let delegatee = options
            .get::<String>("delegatee")
            .unwrap_or_default()
            .trim()
            .to_string();
        if delegatee.is_empty() {
            return Err(crate::ActError::Action(
                "cannot find 'delegatee' in act:delegate".to_string(),
            ));
        }
        self.do_action(pid, tid, EventAction::Delegate, options)
            .await
    }

    /// `act:undelegate {pid,tid}` — revoke the act's delegation.
    pub async fn undelegate(&self, pid: &str, tid: &str, options: Vars) -> Result<()> {
        self.do_action(pid, tid, EventAction::Undelegate, options)
            .await
    }

    /// `act:remind {pid,tid,message}` — a non-owner urges an assigned act.
    /// The kernel validates the caller against the assignment and writes the
    /// reminder record; the act itself is untouched.
    pub async fn remind(&self, pid: &str, tid: &str, options: Vars) -> Result<()> {
        self.do_action(pid, tid, EventAction::Remind, options).await
    }

    /// Run one task action. The action the caller names is the action the
    /// principal is checked against — the wrappers above are spellings of the
    /// same check, never a way around it.
    pub async fn do_action(
        &self,
        pid: &str,
        tid: &str,
        action: EventAction,
        mut options: Vars,
    ) -> Result<()> {
        let name = name_of(&action);
        let moves_assignment = matches!(
            action,
            EventAction::Transfer | EventAction::Delegate | EventAction::Undelegate
        );
        // The shared ACL gate: a per-task action (`act:transfer`,
        // `act:delegate`, `act:undelegate`) passes for an authenticated caller
        // and is decided by the kernel on the task; every other action keeps
        // its global command check. An unauthenticated assignment change is
        // refused here.
        crate::acl::check_action(&self.principal, name)?;
        // The capability is computed from the same policy the global check
        // would apply: `@assign` (or an explicit action) grants it without
        // `@execute`, and a `deny` still wins.
        let can_assign = moves_assignment && self.principal.check(name).is_ok();
        // The assignment revision is the client's compare value, not state:
        // it rides the action envelope and never becomes a task variable. The
        // person and audit fields are task state, never trusted from the
        // request: the caller is the server-attributed subject the action
        // fact persists.
        let rev = take_rev(&mut options)?;
        let mut person_fields = vec![
            "owner",
            "assignee",
            "delegator",
            "subject",
            "updated_by",
            "granted_by",
        ];
        // `delegatee` is the delegate action's *target* (the caller's own
        // choice, like a transfer's `to`), not a claim about the task's
        // current state: every other action's copy is discarded.
        if !matches!(action, EventAction::Delegate) {
            person_fields.push("delegatee");
        }
        for name in person_fields {
            let _ = options.remove(name);
        }
        let subject = (self.principal.is_authenticated() && !self.principal.is_unrestricted())
            .then(|| self.principal.subject().to_string());
        let action = Action::new(pid, tid, action, options)
            .with_subject(subject)
            .with_rev(rev)
            .with_can_assign(can_assign);
        self.runtime.do_action(&action).await
    }
}

/// Pop the client's `rev` compare value: a JSON number or a numeric
/// string (the transports carry options as JSON text). A malformed value is
/// refused like a stale revision, before any durable intent exists.
fn take_rev(options: &mut Vars) -> Result<Option<u64>> {
    let Some(value) = options.remove("rev") else {
        return Ok(None);
    };
    let revision = match &value {
        serde_json::Value::Number(number) => number.as_u64(),
        serde_json::Value::String(text) => text.parse::<u64>().ok(),
        _ => None,
    };
    revision.map(Some).ok_or_else(|| {
        crate::ActError::Action(format!("EASSIGNMENT_CHANGED: invalid rev '{value}'"))
    })
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
        EventAction::Transfer => TRANSFER,
        EventAction::Delegate => DELEGATE,
        EventAction::Undelegate => UNDELEGATE,
        EventAction::Remind => REMIND,
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
            (EventAction::Transfer, "act:transfer"),
            (EventAction::Delegate, "act:delegate"),
            (EventAction::Undelegate, "act:undelegate"),
            (EventAction::Remind, "act:remind"),
        ] {
            assert_eq!(name_of(&action), name);
        }
    }
}
