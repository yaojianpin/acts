use crate::{
    env::VirtualMachine,
    model::{Act, Branch},
    sch::{Matcher, TaskState},
    ShareLock,
};
use serde::{Deserialize, Serialize};
use serde_yaml::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Subject {
    #[serde(default)]
    pub matcher: String,

    #[serde(default)]
    pub users: String,

    #[serde(default)]
    pub on: HashMap<String, Value>,
}

// #[derive(Clone, Default, Serialize, Deserialize)]
// pub struct Action {
//     #[serde(default)]
//     pub name: String,
//     #[serde(default)]
//     pub with: HashMap<String, Value>,
// }

pub type Action = fn(&VirtualMachine);

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Step {
    #[serde(default)]
    pub name: String,

    #[serde(default)]
    pub id: String,

    #[serde(default)]
    pub env: HashMap<String, Value>,

    #[serde(default)]
    pub accept: Option<Value>,

    #[serde(default)]
    pub run: Option<String>,

    #[serde(default)]
    pub on: HashMap<String, Value>,

    #[serde(default)]
    pub r#if: Option<String>,

    #[serde(skip)]
    pub action: Option<Action>,

    #[serde(default)]
    pub branches: Vec<Branch>,

    #[serde(default)]
    pub next: Option<String>,

    #[serde(default)]
    pub subject: Option<Subject>,

    #[serde(skip)]
    pub(crate) acts: ShareLock<Vec<Act>>,

    #[serde(skip)]
    pub(crate) state: ShareLock<TaskState>,
    #[serde(skip)]
    pub(crate) start_time: ShareLock<i64>,
    #[serde(skip)]
    pub(crate) end_time: ShareLock<i64>,

    #[serde(skip)]
    pub(crate) act_ord: ShareLock<usize>,

    #[serde(skip)]
    pub(crate) act_candidates: ShareLock<Vec<Act>>,

    #[serde(skip)]
    pub(crate) act_matcher: ShareLock<Matcher>,
}

impl std::fmt::Debug for Step {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Step")
            .field("name", &self.name)
            .field("id", &self.id)
            .field("env", &self.env)
            .field("accept", &self.accept)
            .field("run", &self.run)
            .field("on", &self.on)
            .field("r#if", &self.r#if)
            .field("branches", &self.branches)
            .field("next", &self.next)
            .field("subject", &self.subject)
            .field("acts", &self.acts)
            .field("state", &self.state)
            .field("start_time", &self.start_time)
            .field("end_time", &self.end_time)
            .field("act_ord", &self.act_ord)
            .field("act_candidates", &self.act_candidates)
            .field("act_matcher", &self.act_matcher)
            .finish()
    }
}
