use super::step::Step;
use crate::{sch::TaskState, ActValue, ShareLock, Workflow};
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

    #[serde(skip)]
    pub(crate) state: ShareLock<TaskState>,
    #[serde(skip)]
    pub(crate) start_time: ShareLock<i64>,
    #[serde(skip)]
    pub(crate) end_time: ShareLock<i64>,

    #[serde(skip)]
    pub(crate) workflow: ShareLock<Box<Workflow>>,
}

impl Job {
    pub fn step(&self, id: &str) -> Option<&Step> {
        match self.steps.iter().find(|step| step.id == id) {
            Some(step) => Some(step),
            None => None,
        }
    }
}
