use crate::Act;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct If {
    #[serde(default)]
    pub on: String,

    #[serde(default)]
    pub then: Vec<Act>,

    #[serde(default)]
    pub r#else: Vec<Act>,
}

impl If {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn with_on(mut self, on: &str) -> Self {
        self.on = on.to_string();
        self
    }

    pub fn with_then(mut self, build: fn(Vec<Act>) -> Vec<Act>) -> Self {
        let stmts = Vec::new();
        self.then = build(stmts);
        self
    }

    pub fn with_else(mut self, build: fn(Vec<Act>) -> Vec<Act>) -> Self {
        let stmts = Vec::new();
        self.r#else = build(stmts);
        self
    }
}

impl From<If> for Act {
    fn from(val: If) -> Self {
        Act::r#if(|_| val.clone())
    }
}
