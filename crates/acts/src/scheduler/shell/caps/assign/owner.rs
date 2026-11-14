//! The assignment state of one human act and its rules.
//!
//! The durable `#assignment` fact ([`Assignment`]) and the rules reading
//! it: who may decide or re-assign an act, and how the assignment projects
//! onto the client payloads — the assignment capability's pure semantics,
//! methods on [`Assign`](super::Assign). The local half of an
//! `assign.owner` expression (literal users, the context variables,
//! user-valued variables, and the set operations) is evaluated by the
//! resolver seam's kernel implementation in the shell; business-data atoms
//! are refused there with a message that points at the
//! `acts.core.users` IRQ package, instead of inventing an owner.

use super::*;
use crate::Result;

/// The private execution variable carrying one human act's assignment:
/// `{"owner","assignee","rev","transferred_from","updated_by",
/// "updated_at"}`. It is the authorization source; the message fields are
/// only its projection.
pub(crate) const ASSIGNMENT_VAR: &str = "#assignment";

/// The assignment state of one human decision execution.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct Assignment {
    pub(crate) owner: String,
    pub(crate) assignee: String,
    #[serde(default)]
    pub(crate) rev: u64,
    #[serde(default)]
    pub(crate) transferred_from: Option<String>,
    #[serde(default)]
    pub(crate) updated_by: Option<String>,
    #[serde(default)]
    pub(crate) updated_at: i64,
}

impl Assignment {
    /// The initial assignment a declared `assign.owner` produces.
    pub(crate) fn new(owner: &str) -> Self {
        Self {
            owner: owner.to_string(),
            assignee: owner.to_string(),
            rev: 0,
            transferred_from: None,
            updated_by: None,
            updated_at: 0,
        }
    }

    /// The aggregate's JSON-text representation (a scope variable value).
    pub(crate) fn value(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// The assignment a transfer to `to` produces: the target becomes both
    /// owner and assignee, the revision advances, and the old owner and the
    /// caller are recorded for audit.
    pub(crate) fn transferred(&self, to: &str, by: Option<&str>, now: i64) -> Self {
        Self {
            owner: to.to_string(),
            assignee: to.to_string(),
            rev: self.rev.saturating_add(1),
            transferred_from: Some(self.owner.clone()),
            updated_by: by.map(str::to_string),
            updated_at: now,
        }
    }

    /// The assignment a delegation to `delegatee` produces: the owner and the
    /// audit trail stay, the assignee becomes the delegatee and the revision
    /// advances. A delegation never moves the act's owner.
    pub(crate) fn delegated(&self, delegatee: &str, by: Option<&str>, now: i64) -> Self {
        Self {
            owner: self.owner.clone(),
            assignee: delegatee.to_string(),
            rev: self.rev.saturating_add(1),
            transferred_from: self.transferred_from.clone(),
            updated_by: by.map(str::to_string),
            updated_at: now,
        }
    }

    /// The assignment revoking a delegation produces: the owner becomes the
    /// assignee again and the revision advances.
    pub(crate) fn revoked(&self, by: Option<&str>, now: i64) -> Self {
        Self {
            owner: self.owner.clone(),
            assignee: self.owner.clone(),
            rev: self.rev.saturating_add(1),
            transferred_from: self.transferred_from.clone(),
            updated_by: by.map(str::to_string),
            updated_at: now,
        }
    }
}

impl Assign {
    /// The owner a sign change (an added/removed approver) must name: the
    /// assigned execution the target belongs to. A pushed act — a plain
    /// pushed act or the leaf of a sign chain — never signs for itself: its
    /// holder (the nearest assigned ancestor) manages the approver set. A
    /// target that is no pushed act starts at itself; the walk then climbs
    /// the parent chain. `None` when no execution on the chain is assigned:
    /// the legacy authority (`@execute`) stays, and a delegatee gains no
    /// signing right either way.
    pub(crate) fn approver_manager(
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        task_id: &crate::scheduler::core::model::task_graph::TaskId,
    ) -> crate::Result<Option<String>> {
        let mut current =
            if crate::scheduler::shell::action::pushed_act_of(snapshot, task_id)?.is_some() {
                snapshot
                    .tasks
                    .get(task_id)
                    .and_then(|task| task.parent.clone())
            } else {
                Some(task_id.clone())
            };
        while let Some(id) = current {
            if let Some(assignment) = owner::Assignment::of(snapshot, &id)? {
                return Ok(Some(assignment.owner));
            }
            current = snapshot.tasks.get(&id).and_then(|task| task.parent.clone());
        }
        Ok(None)
    }

    /// The per-task decision check: a declared assignment admits only its
    /// owner, or a delegatee whose delegation is still valid at `now`; a
    /// decision carrying a revision must name the current one. No assignment
    /// (or no authenticated subject — ACL disabled) keeps the old `@execute`
    /// behavior.
    ///
    /// An expired delegation is a refusal that names the act and the expiry,
    /// never a silent fallback to the assignee: the assignee *is* the expired
    /// delegatee until the decision is refused.
    pub(crate) fn authorize_decision(
        assignment: Option<&Assignment>,
        delegation: Option<&Delegation>,
        subject: Option<&str>,
        rev: Option<u64>,
        node: &str,
        now: i64,
    ) -> Result<()> {
        let (Some(assignment), Some(subject)) = (assignment, subject) else {
            return Ok(());
        };
        if subject != assignment.owner {
            let active = delegation.filter(|delegation| delegation.delegatee == subject);
            match active {
                Some(delegation) if delegation.is_valid(now) => {}
                Some(delegation) => {
                    return Err(crate::ActError::Action(format!(
                        "ENOTOWNER: the delegation of act '{node}' to '{subject}' expired at {}",
                        delegation.expires_at.unwrap_or_default()
                    )));
                }
                None => {
                    return Err(crate::ActError::Action(format!(
                        "ENOTOWNER: user '{subject}' is not the owner or an active delegatee of act '{node}'"
                    )));
                }
            }
        }
        if let Some(rev) = rev
            && rev != assignment.rev
        {
            return Err(crate::ActError::Action(format!(
                "EASSIGNMENT_CHANGED: act '{node}' assignment rev is {}, not {rev}",
                assignment.rev
            )));
        }
        Ok(())
    }

    /// The per-task assignment-change check (transfer, delegate, undelegate):
    /// the current owner, or a caller holding the `assign` capability. A task
    /// without an assignment has no owner to move, so an authenticated caller
    /// is refused rather than silently allowed. ACL disabled (no subject)
    /// keeps the old behavior; the plan itself still refuses an unassigned
    /// act.
    pub(crate) fn authorize_assignment_change(
        assignment: Option<&Assignment>,
        subject: Option<&str>,
        can_assign: bool,
        action: &str,
        node: &str,
    ) -> Result<()> {
        let Some(subject) = subject else {
            return Ok(());
        };
        let Some(assignment) = assignment else {
            return Err(crate::ActError::Action(format!(
                "ENOTOWNER: act '{node}' has no assignment to {action}"
            )));
        };
        if subject == assignment.owner || can_assign {
            return Ok(());
        }
        Err(crate::ActError::Action(format!(
            "ENOTOWNER: user '{subject}' is not the owner of act '{node}' and holds no assign capability"
        )))
    }

    /// The client's revision, when it named one, must be the current
    /// assignment revision: a decision racing a transfer/delegation gets a
    /// refusal naming the revision it actually holds.
    pub(crate) fn compare_rev(assignment: &Assignment, rev: Option<u64>, node: &str) -> Result<()> {
        if let Some(rev) = rev
            && rev != assignment.rev
        {
            return Err(crate::ActError::Action(format!(
                "EASSIGNMENT_CHANGED: act '{node}' assignment rev is {}, not {rev}",
                assignment.rev
            )));
        }
        Ok(())
    }

    /// The act's resolved options with the assignment's routing fields merged
    /// in (what the existing channel globs read). The declared options are
    /// preserved.
    fn options_with_assignment(
        options: serde_json::Value,
        assignment: &Assignment,
    ) -> serde_json::Value {
        let mut map = match options {
            serde_json::Value::Object(map) => map,
            _ => serde_json::Map::new(),
        };
        map.insert(
            "owner".to_string(),
            serde_json::Value::String(assignment.owner.clone()),
        );
        map.insert(
            "assignee".to_string(),
            serde_json::Value::String(assignment.assignee.clone()),
        );
        serde_json::Value::Object(map)
    }

    /// Merge one assignment into an already built act payload. The start
    /// operation uses this for the `Created` record: the payload is built
    /// from the snapshot before the same commit writes `#assignment`.
    pub(crate) fn inject_payload_assignment(
        payload: &mut serde_json::Value,
        assignment: &Assignment,
    ) {
        payload["owner"] = serde_json::Value::String(assignment.owner.clone());
        payload["assignee"] = serde_json::Value::String(assignment.assignee.clone());
        payload["delegator"] = serde_json::Value::Null;
        payload["rev"] = serde_json::json!(assignment.rev);
        payload["ref_msg_id"] = serde_json::Value::Null;
        if let Some(inputs) = payload
            .get_mut("inputs")
            .and_then(|inputs| inputs.as_object_mut())
        {
            let options = inputs.remove("options").unwrap_or(serde_json::Value::Null);
            inputs.insert(
                "options".to_string(),
                Self::options_with_assignment(options, assignment),
            );
        }
    }

    /// Rewrite the assignee (top-level field and its `inputs.options` copy)
    /// on a payload whose owner/rev are already projected.
    pub(crate) fn set_payload_assignee(payload: &mut serde_json::Value, assignee: &str) {
        payload["assignee"] = serde_json::Value::String(assignee.to_string());
        if let Some(options) = payload
            .get_mut("inputs")
            .and_then(|inputs| inputs.get_mut("options"))
            .and_then(|options| options.as_object_mut())
        {
            options.insert(
                "assignee".to_string(),
                serde_json::Value::String(assignee.to_string()),
            );
        }
    }
}
