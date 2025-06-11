use crate::Step;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Catch {
    #[serde(default)]
    pub on: Option<String>,
    #[serde(default)]
    pub steps: Vec<Step>,
}

impl Catch {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn with_on(mut self, err: &str) -> Self {
        self.on = Some(err.to_string());
        self
    }

    pub fn with_step(mut self, build: fn(Step) -> Step) -> Self {
        let step = build(Step::default());
        self.steps.push(step);

        self
    }
}
