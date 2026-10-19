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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catch_builder_defaults_and_setters() {
        let catch = Catch::new();
        assert!(catch.on.is_none());
        assert!(catch.steps.is_empty());

        let catch = catch.with_on("err1").with_step(|step| step.with_id("s1"));
        assert_eq!(catch.on.as_deref(), Some("err1"));
        assert_eq!(catch.steps.len(), 1);
        assert_eq!(catch.steps[0].id, "s1");
    }
}
