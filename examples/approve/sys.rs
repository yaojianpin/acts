//! The demo's business system: the directory (roles, units, departments)
//! and every answer that needs no user — the candidate pools
//! (`acts.core.users`) and the delegated join verdicts (`acts.core.decide`).
//! `Sys::process` runs ahead of the interactive client on every message:
//! what it can answer, it answers; what it cannot, it hands on.

use acts::{Executor, Message, MessageState, Result, Vars};
use serde_json::json;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct User<'a> {
    pub id: &'a str,
    pub name: &'a str,
}

/// One participant the demo directory answers with (the contract's
/// `{ id, code, name }`, code mirroring the id).
#[derive(Debug, Clone)]
struct Candidate {
    id: String,
    name: String,
}

impl Candidate {
    fn of(user: &User<'_>) -> Self {
        Self {
            id: user.id.to_string(),
            name: user.name.to_string(),
        }
    }

    fn named(id: &str) -> Self {
        Self {
            id: id.to_string(),
            name: id.to_string(),
        }
    }
}

/// The demo's business system.
#[derive(Default)]
pub struct Sys<'a> {
    /// role → the users holding it (the pm / gm pools).
    users: HashMap<String, Vec<User<'a>>>,
    /// (resource kind, name) → its people, the principal (the post holder)
    /// first. This is the demo's business directory for `owners_of` atoms.
    resources: HashMap<(String, String), Vec<User<'a>>>,
}

impl<'a> Sys<'a> {
    pub fn new() -> Self {
        Sys {
            ..Default::default()
        }
    }

    pub fn init(&mut self) {
        // the pm role: John and Rose
        self.add_user(
            User {
                id: "1",
                name: "John",
            },
            "pm",
        );
        self.add_user(
            User {
                id: "2",
                name: "Rose",
            },
            "pm",
        );
        // the gm role: Tom and Amy — `join: any` passes on the first vote,
        // so asking both demonstrates the early decision
        self.add_user(
            User {
                id: "3",
                name: "Tom",
            },
            "gm",
        );
        self.add_user(
            User {
                id: "4",
                name: "Amy",
            },
            "gm",
        );
        // Units and departments: the first user is the post's principal.
        // Lily holds both the department's and the unit's post, so the post
        // approval's pool (the union of both principals) deduplicates to her
        // single vote — the concurrent post approves once.
        self.add_unit(
            "rd_center",
            &[
                User {
                    id: "6",
                    name: "Lily",
                },
                User {
                    id: "5",
                    name: "Ken",
                },
            ],
        );
        self.add_department(
            "rd",
            &[
                User {
                    id: "6",
                    name: "Lily",
                },
                User {
                    id: "5",
                    name: "Ken",
                },
            ],
        );
        self.add_department(
            "finance",
            &[
                User {
                    id: "7",
                    name: "Mike",
                },
                User {
                    id: "4",
                    name: "Amy",
                },
            ],
        );
    }

    pub fn add_user(&mut self, user: User<'a>, role: &str) {
        self.users
            .entry(role.to_string())
            .and_modify(|v| v.push(user.clone()))
            .or_insert(vec![user.clone()]);
    }

    /// Register a business unit: the first user is its principal.
    pub fn add_unit(&mut self, unit: &str, users: &[User<'a>]) {
        self.add_resource("unit", unit, users);
    }

    /// Register a department: the first user is its principal.
    pub fn add_department(&mut self, department: &str, users: &[User<'a>]) {
        self.add_resource("department", department, users);
    }

    fn add_resource(&mut self, kind: &str, name: &str, users: &[User<'a>]) {
        self.resources
            .insert((kind.to_string(), name.to_string()), users.to_vec());
    }

    /// Every user id the directory knows — the demo actors the interactive
    /// client builds principals for.
    pub fn users(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .users
            .values()
            .flatten()
            .map(|user| user.id.to_string())
            .collect();
        for users in self.resources.values() {
            ids.extend(users.iter().map(|user| user.id.to_string()));
        }
        ids.sort();
        ids.dedup();
        ids
    }

    /// Answer everything that needs no user. `Ok(true)` — the message was
    /// handled; `Ok(false)` — hand it to the interactive client.
    pub async fn process(&self, executor: &Executor, message: &Message) -> Result<bool> {
        println!(
            "##########message: type={} state={} pid={} tid={} owner={:?} inputs={}",
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
                .unwrap_or_default();
            let candidates = self.candidates(&params);
            let participants: Vec<serde_json::Value> = candidates
                .iter()
                .map(|candidate| {
                    json!({
                        "id": candidate.id,
                        "code": candidate.id,
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

    /// Resolve one prepared candidate expression against the demo directory.
    ///
    /// The engine has already replaced every local atom (`${{ }}` strings,
    /// `initiator`/`current_actor`/… and user variables) with `user` nodes,
    /// so the client only answers what only it knows: roles, unit/department
    /// owners and the set operations.
    fn candidates(&self, expr: &serde_json::Value) -> Vec<Candidate> {
        match expr {
            serde_json::Value::String(id) => vec![Candidate::named(id)],
            serde_json::Value::Array(items) => items
                .iter()
                .flat_map(|item| self.candidates(item))
                .collect(),
            serde_json::Value::Object(map) => {
                let kind = map
                    .get("kind")
                    .and_then(|kind| kind.as_str())
                    .unwrap_or_default();
                match kind {
                    // a local atom the engine already resolved
                    "user" => map
                        .get("name")
                        .and_then(|name| name.as_str())
                        .map(Candidate::named)
                        .into_iter()
                        .collect(),
                    "role" => map
                        .get("name")
                        .and_then(|name| name.as_str())
                        .and_then(|name| self.users.get(name))
                        .map(|users| users.iter().map(Candidate::of).collect())
                        .unwrap_or_default(),
                    "owners_of" => self.owners_of(map),
                    "any_of" => self.union(map),
                    "all_of" => self.intersect(map),
                    "except" => self.except(map),
                    // a business variable the demo directory does not model
                    _ => Vec::new(),
                }
            }
            _ => Vec::new(),
        }
    }

    /// `owners_of`: a resource's people by duty — `principal` is the
    /// directory's first member; `all`/absent (and `member`, which the demo
    /// does not separate from `all`) answers the full list.
    fn owners_of(&self, expr: &serde_json::Map<String, serde_json::Value>) -> Vec<Candidate> {
        let Some(resource) = expr.get("resource") else {
            return Vec::new();
        };
        let kind = resource
            .get("resource")
            .and_then(|kind| kind.as_str())
            .unwrap_or_default();
        let name = resource
            .get("name")
            .and_then(|name| name.as_str())
            .unwrap_or_default();
        let Some(members) = self.resources.get(&(kind.to_string(), name.to_string())) else {
            return Vec::new();
        };
        match expr
            .get("duty")
            .and_then(|duty| duty.as_str())
            .unwrap_or("all")
        {
            "principal" => members
                .first()
                .map(|user| vec![Candidate::of(user)])
                .unwrap_or_default(),
            _ => members.iter().map(Candidate::of).collect(),
        }
    }

    fn operands(&self, expr: &serde_json::Map<String, serde_json::Value>) -> Vec<Vec<Candidate>> {
        expr.get("operands")
            .and_then(|operands| operands.as_array())
            .map(|operands| {
                operands
                    .iter()
                    .map(|operand| self.candidates(operand))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn union(&self, expr: &serde_json::Map<String, serde_json::Value>) -> Vec<Candidate> {
        let mut seen = HashSet::new();
        let mut candidates = Vec::new();
        for operand in self.operands(expr) {
            for candidate in operand {
                if seen.insert(candidate.id.clone()) {
                    candidates.push(candidate);
                }
            }
        }
        candidates
    }

    fn intersect(&self, expr: &serde_json::Map<String, serde_json::Value>) -> Vec<Candidate> {
        let mut operands = self.operands(expr).into_iter();
        let Some(mut selected) = operands.next() else {
            return Vec::new();
        };
        for other in operands {
            let ids: HashSet<String> = other.into_iter().map(|candidate| candidate.id).collect();
            selected.retain(|candidate| ids.contains(&candidate.id));
        }
        selected
    }

    fn except(&self, expr: &serde_json::Map<String, serde_json::Value>) -> Vec<Candidate> {
        let base = expr
            .get("base")
            .map(|base| self.candidates(base))
            .unwrap_or_default();
        let minus = expr
            .get("minus")
            .map(|minus| self.candidates(minus))
            .unwrap_or_default();
        let minus: HashSet<&str> = minus
            .iter()
            .map(|candidate| candidate.id.as_str())
            .collect();
        base.into_iter()
            .filter(|candidate| !minus.contains(candidate.id.as_str()))
            .collect()
    }
}
