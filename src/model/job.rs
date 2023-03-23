use super::step::Step;
use crate::{ActValue, ModelBase};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Job {
    #[serde(default)]
    pub id: String,

    #[serde(default)]
    pub name: String,

    #[serde(default)]
    pub steps: Vec<Step>,

    #[serde(default)]
    pub needs: Vec<String>,

    #[serde(default)]
    pub env: HashMap<String, ActValue>,

    #[serde(default)]
    pub accept: Option<ActValue>,

    #[serde(default)]
    pub outputs: HashMap<String, ActValue>,

    #[serde(default)]
    pub on: HashMap<String, ActValue>,
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
