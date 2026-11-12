//! Organization structure example: unit / department / person / role.
//!
//! Modeling notes:
//! - `Unit` aggregates all `Person` and `Department` values.
//! - `Department.parent_id` expresses the parent department.
//! - `Department.manager_id` expresses the department head.
//! - `Department.member_ids` expresses department members.
//! - `Person.role` expresses the person's role.

use acts::{
    ActError, CandidateExpr, ContextVar, Duty, Executor, Message, MessageState, Participant,
    Relation, RelationName, ResourceRef, VarType, Vars,
};
use anyhow::{Result, bail};
use serde_json::json;
use std::collections::{HashMap, HashSet};

mod dept;
mod person;
mod role;
mod unit;

use dept::{Department, DeptId, DeptKind};
use person::{Person, UserId};
use role::Role;
use unit::Unit;

/// The demo's business system.
#[derive(Debug)]
pub struct Sys {
    unit: Unit,
}

#[derive(Debug, Clone)]
pub enum RelTarget {
    Person(Participant),
    Resource(ResourceRef),
}

fn to_participant(p: &Person) -> Participant {
    Participant::with_names(&p.id.to_string(), &p.id.to_string(), &p.name)
}

fn dedup(mut v: Vec<Participant>) -> Vec<Participant> {
    v.sort_by(|a, b| a.id.cmp(&b.id));
    v.dedup_by(|a, b| a.id == b.id);
    v
}

fn map_err(err: ActError) -> anyhow::Error {
    anyhow::anyhow!("act error: {err}")
}

impl Sys {
    pub fn new() -> Self {
        Sys {
            unit: Unit::build(),
        }
    }

    pub fn users(&self) -> Vec<String> {
        self.unit.people.iter().map(|p| p.id.to_string()).collect()
    }

    /// Answer everything that needs no user. `Ok(true)` — the message was
    /// handled; `Ok(false)` — hand it to the interactive client.
    pub async fn process(&self, executor: &Executor, message: &Message) -> Result<bool> {
        println!(
            "########## message: type={} state={} pid={} tid={} owner={:?} inputs={}",
            message.r#type,
            message.state(),
            message.pid,
            message.tid,
            message.owner,
            message.inputs
        );
        // A candidates request — `assign.candidates`'s pool, or a step that
        // uses the package directly: resolve the prepared expression against
        // the demo directory and answer the contract
        // `{ participants, ids, count }`.
        if message.is_uses("acts.core.users") && message.is_state(MessageState::Created) {
            let params = message
                .inputs
                .get::<serde_json::Value>("params")
                .ok_or_else(|| anyhow::anyhow!("missing 'params' in message inputs"))?;

            println!("candidates request: params={params}");

            let expr = serde_json::from_value(params).map_err(|error| {
                anyhow::anyhow!(
                    "cannot parse the candidate expression from the message inputs: {error}"
                )
            })?;

            let candidates = self.eval(&expr)?;
            let participants: Vec<serde_json::Value> = candidates
                .iter()
                .map(|candidate| {
                    json!({
                        "id": candidate.id,
                        "code": candidate.code,
                        "name": candidate.name,
                    })
                })
                .collect();
            let ids: Vec<&str> = candidates
                .iter()
                .map(|candidate| candidate.id.as_str())
                .collect();
            if let Err(err) = self
                .complete(
                    executor,
                    &message.pid,
                    &message.tid,
                    &[
                        ("participants", json!(participants)),
                        ("ids", json!(ids)),
                        ("count", json!(ids.len())),
                    ],
                )
                .await
            {
                // A rewind (a cancel upstream) can close this request while
                // the answer is in flight: the refusal is not an error for
                // the business system.
                println!("candidates answer ignored: {err}");
            }
            return Ok(true);
        }

        // The delegated join request (`join: { params, uses }`): the engine
        // asks the client to decide the step, one request per vote that
        // lands. The board approval passes as soon as the chair approved;
        // until then the request is skipped — deferred — and the engine asks
        // again when the next vote arrives.
        if message.is_uses("acts.core.decide") && message.is_state(MessageState::Created) {
            // The kernel's envelope: `{ params: <rule params>, outcomes }` —
            // the package gives `params` its meaning, and the business
            // handler key rides `params.action`.
            let envelope = message.inputs.get::<Vars>("params").unwrap_or_default();
            let rule = envelope.get::<Vars>("params").unwrap_or_default();
            if rule.get::<String>("action").as_deref() == Some("board_decision") {
                let outcomes = envelope
                    .get::<serde_json::Value>("outcomes")
                    .unwrap_or_default();
                let chair_passed = outcomes.as_array().is_some_and(|outcomes| {
                    outcomes.iter().any(|outcome| {
                        outcome["outputs"]["role"] == json!("chair")
                            && outcome["state"] == json!("completed")
                    })
                });
                let verdict = if chair_passed {
                    self.complete(executor, &message.pid, &message.tid, &[])
                        .await
                } else {
                    executor
                        .act()
                        .skip(&message.pid, &message.tid, Vars::new())
                        .await
                        .map_err(map_err)
                };
                if let Err(err) = verdict {
                    // A racing verdict already decided the step: the other
                    // requests were withdrawn.
                    println!("join request ignored: {err}");
                }
                return Ok(true);
            }
            return Ok(true);
        }

        Ok(false)
    }

    async fn complete(
        &self,
        executor: &Executor,
        pid: &str,
        tid: &str,
        options: &[(&str, serde_json::Value)],
    ) -> Result<()> {
        let mut vars = Vars::new();
        for (name, value) in options {
            vars.insert(name.to_string(), value.clone());
        }
        executor.act().complete(pid, tid, vars).await?;
        println!("completed id={tid}");
        Ok(())
    }

    fn members(&self, dept: &Department) -> Vec<Participant> {
        dept.member_ids
            .iter()
            .filter_map(|id| self.unit.person(id).map(to_participant))
            .collect()
    }

    pub fn users_by_id(&self, id: &str) -> Option<Participant> {
        self.unit.person(id).map(to_participant)
    }

    pub fn dept_by_id(&self, id: impl Into<DeptId>) -> Option<&Department> {
        let id = id.into();
        self.unit.departments.iter().find(|d| d.id == id)
    }

    /// Resolves a model's department reference: a department id, its display
    /// name or its kind label — a model names resources the way the business
    /// does (`name: Finance Department`), while the relation path carries
    /// resolved ids.
    pub fn dept_by_ref(&self, reference: &str) -> Option<&Department> {
        let reference = reference.trim();
        self.dept_by_id(reference)
            .or_else(|| {
                self.unit
                    .departments
                    .iter()
                    .find(|d| d.name.eq_ignore_ascii_case(reference))
            })
            .or_else(|| {
                self.unit
                    .departments
                    .iter()
                    .find(|d| d.kind.to_string() == reference)
            })
    }

    pub fn users_by_role(&self, role: &str) -> Vec<Participant> {
        let Some(role) = Role::from_label(role) else {
            return Vec::new();
        };
        self.unit
            .people
            .iter()
            .filter(|p| p.role == role)
            .map(to_participant)
            .collect()
    }

    pub fn users_by_group(&self, group: &str) -> Vec<Participant> {
        let kind: DeptKind = group.parse().unwrap_or(DeptKind::ExecutiveOffice);
        self.unit
            .dept_by_kind(kind)
            .map(|d| self.members(d))
            .unwrap_or_default()
    }

    pub fn resource_people(&self, res: &ResourceRef, duty: &Duty) -> Vec<Participant> {
        match res {
            ResourceRef::Unit { name } => {
                if name != &self.unit.name {
                    return Vec::new();
                }
                match duty {
                    // All members of the unit.
                    Duty::All | Duty::Member => {
                        self.unit.people.iter().map(to_participant).collect()
                    }
                    // The unit principal is the head of the executive office.
                    Duty::Principal => self
                        .unit
                        .departments
                        .iter()
                        .find(|d| d.kind == DeptKind::ExecutiveOffice)
                        .map(|d| self.manager(d))
                        .unwrap_or_default(),
                    // No deputy/custom duties modeled locally.
                    Duty::Deputy | Duty::Custom(_) => Vec::new(),
                }
            }
            ResourceRef::Department { name } => {
                let Some(dept) = self.dept_by_ref(name) else {
                    return Vec::new();
                };
                match duty {
                    Duty::Principal => self.manager(dept),
                    Duty::All | Duty::Member => self.members(dept),
                    // A deployment duty: the directory decides what the word
                    // means. This directory's `assistant` selects the
                    // department's members with the assistant role — the
                    // Executive Office's one assistant.
                    Duty::Custom(duty) if duty == "assistant" => dept
                        .member_ids
                        .iter()
                        .filter_map(|id| self.unit.person(id))
                        .filter(|p| p.role == Role::Assistant)
                        .map(to_participant)
                        .collect(),
                    // No deputy/custom duties modeled locally.
                    Duty::Deputy | Duty::Custom(_) => Vec::new(),
                }
            }
            // No projects in this model.
            ResourceRef::Project { .. } => Vec::new(),
        }
    }

    fn relate(&self, from: &Participant, rel: &Relation) -> Vec<RelTarget> {
        let Some(name) = rel.as_builtin() else {
            return Vec::new();
        };
        let Some(uid) = from.id.parse::<UserId>().ok() else {
            return Vec::new();
        };
        let Some(dept) = self.dept_of(&uid) else {
            return Vec::new();
        };

        match name {
            RelationName::DepartmentOf => {
                vec![RelTarget::Resource(ResourceRef::Department {
                    name: dept.id.clone(),
                })]
            }
            RelationName::UnitOf => vec![RelTarget::Resource(self.unit_ref())],
            RelationName::ProjectOf => Vec::new(),
            RelationName::ManagerOf => {
                // The person's manager: the department head, unless the person
                // is the head, in which case the parent department's head.
                let manager_id = if dept.manager_id == Some(uid.clone()) {
                    dept.parent_id
                        .as_ref()
                        .and_then(|p| self.dept_by_id(p))
                        .and_then(|parent| parent.manager_id.clone())
                } else {
                    dept.manager_id.clone()
                };
                manager_id
                    .and_then(|id| self.unit.person(&id))
                    .map(|p| vec![RelTarget::Person(to_participant(p))])
                    .unwrap_or_default()
            }
            RelationName::ReportsOf => {
                // Direct reports: members of departments this person manages.
                self.unit
                    .departments
                    .iter()
                    .filter(|d| d.manager_id == Some(uid.clone()))
                    .flat_map(|d| d.member_ids.iter().cloned())
                    .filter(|id| *id != uid)
                    .filter_map(|id| self.unit.person(&id).map(to_participant))
                    .map(RelTarget::Person)
                    .collect()
            }
            // No assistant relation in this model.
            RelationName::AssistantOf => Vec::new(),
        }
    }

    fn manager(&self, dept: &Department) -> Vec<Participant> {
        dept.manager_id
            .as_ref()
            .and_then(|id| self.unit.person(id))
            .map(|p| vec![to_participant(p)])
            .unwrap_or_default()
    }

    fn dept_of(&self, id: &str) -> Option<&Department> {
        self.unit
            .departments
            .iter()
            .find(|d| d.member_ids.contains(&id.to_string()))
    }

    fn unit_ref(&self) -> ResourceRef {
        ResourceRef::Unit {
            name: self.unit.name.clone(),
        }
    }

    fn eval(&self, expr: &CandidateExpr) -> Result<Vec<Participant>> {
        match expr {
            CandidateExpr::Value(s) => Ok(vec![
                self.users_by_id(s)
                    .ok_or_else(|| anyhow::anyhow!("user not found: {s}"))?,
            ]),
            CandidateExpr::User { value: name } => {
                Ok(vec![self.users_by_id(name).ok_or_else(|| {
                    anyhow::anyhow!("user not found: {name}")
                })?])
            }
            CandidateExpr::Role { name } => {
                if Role::from_label(name).is_none() {
                    bail!("invalid role name: {name}")
                }
                Ok(self.users_by_role(name))
            }
            CandidateExpr::Group { name } => Ok(self.users_by_group(name)),
            CandidateExpr::OwnersOf { resource, duty } => Ok(self.resource_people(resource, duty)),
            CandidateExpr::Context { var } => self.eval_context(var),
            CandidateExpr::Relate { from, rel, duty } => {
                let people = self.eval(from)?;
                let mut out = Vec::new();
                for person in people {
                    for target in self.relate(&person, rel) {
                        match target {
                            RelTarget::Person(p) => out.push(p),
                            RelTarget::Resource(res) => {
                                let d = duty.clone().unwrap_or(Duty::All);
                                out.extend(self.resource_people(&res, &d));
                            }
                        }
                    }
                }
                Ok(dedup(out))
            }
            CandidateExpr::AnyOf { operands } => {
                let mut out = Vec::new();
                for op in operands {
                    out.extend(self.eval(op)?);
                }
                Ok(dedup(out))
            }
            CandidateExpr::AllOf { operands } => {
                let mut iter = operands.iter();
                let Some(first) = iter.next() else {
                    return Ok(Vec::new());
                };
                let mut acc: HashSet<String> =
                    self.eval(first)?.into_iter().map(|p| p.id).collect();
                let mut by_id: HashMap<String, Participant> = HashMap::new();
                for p in self.eval(first)?.into_iter() {
                    by_id.entry(p.id.clone()).or_insert(p);
                }
                for op in iter {
                    let next: HashSet<String> = self.eval(op)?.into_iter().map(|p| p.id).collect();
                    acc.retain(|id| next.contains(id));
                }
                let out = acc.into_iter().filter_map(|id| by_id.remove(&id)).collect();
                Ok(dedup(out))
            }
            CandidateExpr::Except { base, minus } => {
                let base_people = self.eval(base)?;
                let minus_people = self.eval(minus)?;
                let minus_ids: HashSet<String> = minus_people.into_iter().map(|p| p.id).collect();
                let out = base_people
                    .into_iter()
                    .filter(|p| !minus_ids.contains(&p.id))
                    .collect();
                Ok(dedup(out))
            }
        }
    }

    fn eval_context(&self, var: &ContextVar) -> Result<Vec<Participant>> {
        match var {
            ContextVar::Initiator
            | ContextVar::CurrentActor
            | ContextVar::PreviousApprover
            | ContextVar::PreviousApproverNth { .. } => {
                bail!(
                    "cannot evaluate context variable {var:?}, the var should be resolved by the engine before reaching the business system"
                )
            }

            ContextVar::Var { name, ty, duty } => match ty {
                VarType::User | VarType::Users => bail!(
                    "cannot evaluate context variable {var:?}, the var should be resolved by the engine before reaching the business system"
                ),
                VarType::Role => Ok(self.users_by_role(name)),
                VarType::Group => Ok(self.users_by_group(name)),
                VarType::Department => {
                    Ok(self.resource_people(&ResourceRef::Department { name: name.clone() }, duty))
                }
                VarType::Unit => {
                    Ok(self.resource_people(&ResourceRef::Unit { name: name.clone() }, duty))
                }
                VarType::Project => bail!(
                    "The business system does not implement projects, cannot evaluate context variable {var:?}"
                ),
            },
        }
    }
}
