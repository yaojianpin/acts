use crate::{
    model::{Act, Branch},
    ModelBase, Vars,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Step {
    #[serde(default)]
    pub name: String,

    #[serde(default)]
    pub id: String,

    #[serde(default)]
    pub inputs: Vars,

    #[serde(default)]
    pub outputs: Vars,

    #[serde(default)]
    pub tag: String,

    #[serde(default)]
    pub run: Option<String>,

    #[serde(default)]
    pub r#if: Option<String>,

    #[serde(default)]
    pub branches: Vec<Branch>,

    #[serde(default)]
    pub next: Option<String>,

    #[serde(default)]
    pub acts: Vec<Act>,
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
            .field("env", &self.inputs)
            .field("run", &self.run)
            // .field("on", &self.on)
            .field("r#if", &self.r#if)
            .field("branches", &self.branches)
            .field("next", &self.next)
            .field("acts", &self.acts)
            // .field("subject", &self.subject)
            .finish()
    }
}
