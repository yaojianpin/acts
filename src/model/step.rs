use crate::{
    env::VirtualMachine,
    model::{Act, Branch},
    sch::Matcher,
    ModelBase, ShareLock,
};
use serde::{Deserialize, Serialize};
use serde_yaml::Value;
use std::collections::HashMap;
use std::sync::RwLockWriteGuard;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Subject {
    #[serde(default)]
    pub matcher: String,

    #[serde(default)]
    pub users: String,

    #[serde(default)]
    pub on: HashMap<String, Value>,
}

#[derive(Debug, Clone, Default)]
pub struct Cands {
    pub matcher: Matcher,
    pub acts: Vec<Act>,
    pub ord: usize,
}

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
    pub(crate) cands: ShareLock<Cands>,
}

impl Step {
    pub fn acts(&self) -> Vec<Act> {
        self.acts.read().unwrap().clone()
    }

    pub(crate) fn push_act(&self, act: &Act) {
        self.acts.write().unwrap().push(act.clone());
    }

    pub(crate) fn cands(&self) -> RwLockWriteGuard<Cands> {
        self.cands.as_ref().write().unwrap()
    }
}

impl ModelBase for Step {
    fn id(&self) -> &str {
        &self.id
    }
}

impl std::fmt::Debug for Step {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Step")
            .field("name", &self.name)
            .field("id", &self.id)
            .field("env", &self.env)
            .field("run", &self.run)
            .field("on", &self.on)
            .field("r#if", &self.r#if)
            .field("branches", &self.branches)
            .field("next", &self.next)
            .field("subject", &self.subject)
            .field("acts", &self.acts)
            .field("cands", &self.cands)
            .finish()
    }
}
