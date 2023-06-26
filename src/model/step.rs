use crate::{model::Branch, ActValue, ModelBase, Vars};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Subject {
    #[serde(default)]
    pub matcher: String,

    #[serde(default)]
    pub cands: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OnCallback {
    #[serde(default)]
    pub task: HashMap<String, ActValue>,
    #[serde(default)]
    pub act: HashMap<String, ActValue>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Step {
    #[serde(default)]
    pub name: String,

    #[serde(default)]
    pub id: String,

    #[serde(default)]
    pub env: Vars,

    #[serde(default)]
    pub run: Option<String>,

    #[serde(default)]
    pub on: Option<OnCallback>,

    #[serde(default)]
    pub r#if: Option<String>,

    #[serde(default)]
    pub branches: Vec<Branch>,

    #[serde(default)]
    pub next: Option<String>,

    #[serde(default)]
    pub subject: Option<Subject>,

    #[serde(default)]
    pub action: Option<String>,
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
            .finish()
    }
}
