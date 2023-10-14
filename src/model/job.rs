use super::step::Step;
use crate::{ModelBase, Vars};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Job {
    #[serde(default)]
    pub id: String,

    #[serde(default)]
    pub name: String,

    #[serde(default)]
    pub tag: String,

    #[serde(default)]
    pub steps: Vec<Step>,

    #[serde(default)]
    pub needs: Vec<String>,

    #[serde(default)]
    pub inputs: Vars,

    #[serde(default)]
    pub outputs: Vars,
}

impl Job {
    pub fn step(&self, id: &str) -> Option<&Step> {
        match self.steps.iter().find(|step| step.id == id) {
            Some(step) => Some(step),
            None => None,
        }
    }
}

impl ModelBase for Job {
    fn id(&self) -> &str {
        &self.id
    }
}
