use acts::{Executor, Message, MessageState, Result, Vars};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Clone)]
pub struct User<'a> {
    pub id: &'a str,
    #[allow(dead_code)]
    pub name: &'a str,
}

#[derive(Debug, Default)]
pub struct Client<'a> {
    users: HashMap<String, Vec<User<'a>>>,
    /// The `pm_review` act, held so the countersign flow can answer it after
    /// the added approver is dropped: `pm_review` is pushed an added approver
    /// on its own `Created`, and only completed once that push is removed.
    pm_review: Mutex<Option<(String, String)>>,
}

impl<'a> Client<'a> {
    pub fn new() -> Self {
        Client {
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
    }

    pub fn add_user(&mut self, user: User<'a>, role: &str) {
        self.users
            .entry(role.to_string())
            .and_modify(|v| v.push(user.clone()))
            .or_insert(vec![user.clone()]);
    }

    pub async fn process(&self, executor: &Executor, message: &Message) -> Result<()> {
        // The delegated join request (`join: { action, … }`): the engine asks
        // the client to decide the step, one request per vote that lands. The
        // board countersign passes as soon as the chair approved; until
        // then the request is skipped — deferred — and the
        // engine asks again when the next vote arrives.
        if message.is_uses("acts.core.decide") && message.is_state(MessageState::Created) {
            let params = message.inputs.get::<Vars>("params").unwrap_or_default();
            if params.get::<String>("action").as_deref() == Some("board_decision") {
                let outcomes = params
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
                return Ok(());
            }
            return Ok(());
        }

        if let Some(params) = message.inputs.get::<Vars>("params")
            && let Some(key) = params.get::<String>("key")
        {
            println!(
                "act: key={key} nid={} state={}",
                message.nid,
                message.state()
            );
            match key.as_str() {
                // step 1 · prepare: complete it at once
                "prepare" if message.is_state(MessageState::Created) => {
                    self.complete(
                        executor,
                        &message.pid,
                        &message.tid,
                        &[("uid", json!("u1"))],
                    )
                    .await?;
                }
                // step 2 · pm approval, the review act: push an added approver
                // under it (the countersign push) and hold the answer until
                // that push is dropped again (the countersign removal)
                "pm_review" if message.is_state(MessageState::Created) => {
                    *self.pm_review.lock().unwrap() =
                        Some((message.pid.clone(), message.tid.clone()));
                    executor
                        .act()
                        .push(
                            &message.pid,
                            &message.tid,
                            Vars::new()
                                .with("name", "added approver")
                                .with("uses", "acts.core.irq")
                                .with("params", Vars::new().with("key", "pm_added")),
                        )
                        .await?;
                    println!("pushed an added approver under {}", message.tid);
                }
                // step 2 · pm approval, the pushed act: remove it (the
                // countersign removal), then answer the review act with the
                // pm user list
                "pm_added" if message.is_state(MessageState::Created) => {
                    executor
                        .act()
                        .remove(&message.pid, &message.tid, Vars::new())
                        .await?;
                    println!("removed the added approver {}", message.tid);
                    let approval = self.pm_review.lock().unwrap().clone();
                    if let Some((pid, tid)) = approval {
                        self.complete(
                            executor,
                            &pid,
                            &tid,
                            &[("uid", json!("admin")), ("pm", json!(self.role("pm")))],
                        )
                        .await?;
                    }
                }
                // step 2 · pm approval, one vote per pm user
                "pm_vote" if message.is_state(MessageState::Created) => {
                    self.complete(
                        executor,
                        &message.pid,
                        &message.tid,
                        &[("uid", json!("u1"))],
                    )
                    .await?;
                }
                // step 3 · gm approval, the review act: answer with the gm
                // user list
                "gm_review" if message.is_state(MessageState::Created) => {
                    self.complete(
                        executor,
                        &message.pid,
                        &message.tid,
                        &[("uid", json!("admin")), ("gm", json!(self.role("gm")))],
                    )
                    .await?;
                }
                // step 3 · gm approval, one vote per gm user: the first
                // answer passes the step (`join: any`); the rest of the
                // users' votes were withdrawn by then, so a refused answer
                // is expected and ignored
                "gm_vote" if message.is_state(MessageState::Created) => {
                    self.try_complete(
                        executor,
                        &message.pid,
                        &message.tid,
                        &[("uid", json!("u2"))],
                    )
                    .await;
                }
                // step 4 · board countersign, the chair: approve, and say who
                // voted — the join handler reads the role from the outputs
                "board_chair" if message.is_state(MessageState::Created) => {
                    self.try_complete(
                        executor,
                        &message.pid,
                        &message.tid,
                        &[("role", json!("chair"))],
                    )
                    .await;
                }
                // step 4 · board countersign, the deputy: reject — the chair's
                // approval still passes the step (the delegated join decides)
                "board_deputy" if message.is_state(MessageState::Created) => {
                    if let Err(err) = executor
                        .act()
                        .fail(
                            &message.pid,
                            &message.tid,
                            Vars::new()
                                .with("ecode", "rejected")
                                .with("message", "the deputy rejected"),
                        )
                        .await
                    {
                        println!("deputy vote ignored: {err}");
                    }
                }
                _ => {}
            }
        }

        if message.is_type("msg") {
            println!("msg: id={} inputs={}", message.tid, message.inputs);
        }

        Ok(())
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

    /// Complete an act the join may already have withdrawn: a racing answer
    /// is refused by the engine and is not an error for this demo. A `join:
    /// any` step passes on the first vote, so the other voters' answers are
    /// refused exactly that way.
    async fn try_complete(
        &self,
        executor: &Executor,
        pid: &str,
        tid: &str,
        options: &[(&str, serde_json::Value)],
    ) {
        if let Err(err) = self.complete(executor, pid, tid, options).await {
            println!("vote ignored: {err}");
        }
    }
}

impl Client<'_> {
    /// The user ids of one role: what a review act's answer carries.
    fn role(&self, name: &str) -> Vec<String> {
        let mut ret = vec![];
        if let Some(users) = self.users.get(name) {
            let iter = users.iter().map(|u| u.id.to_string());
            ret.extend(iter);
        }
        ret
    }
}
