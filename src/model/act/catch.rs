use crate::{Act, Vars};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Catch {
    #[serde(default)]
    pub on: Option<String>,
    #[serde(default)]
    pub inputs: Vars,
    #[serde(default)]
    pub then: Vec<Act>,
}

impl Catch {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn with_on(mut self, err: &str) -> Self {
        self.on = Some(err.to_string());
        self
    }

    pub fn with_error(mut self, err: &str) -> Self {
        self.inputs.set("error", err.to_string());
        self
    }

    pub fn with_then(mut self, build: fn(Vec<Act>) -> Vec<Act>) -> Self {
        let stmts = Vec::new();
        self.then = build(stmts);

        self
    }
}

impl From<Catch> for Act {
    fn from(val: Catch) -> Self {
        Act::catch(|_| val.clone())
    }
}
