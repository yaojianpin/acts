use crate::{model::Step, ModelBase, Vars};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Branch {
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

    pub r#if: Option<String>,

    #[serde(default)]
    pub steps: Vec<Step>,

    #[serde(default)]
    pub next: Option<String>,

    #[serde(default)]
    pub default: bool,

    #[serde(default)]
    pub needs: Vec<String>,
}

impl ModelBase for Branch {
    fn id(&self) -> &str {
        &self.id
    }
}
