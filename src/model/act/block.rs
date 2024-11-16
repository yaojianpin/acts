use crate::{Act, Vars};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Block {
    #[serde(default)]
    pub then: Vec<Act>,

    #[serde(default)]
    pub inputs: Vars,

    #[serde(default)]
    pub next: Option<Box<Act>>,
}

impl Block {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn with_input<T>(mut self, name: &str, value: T) -> Self
    where
        T: Serialize + Clone,
    {
        self.inputs.set(name, value);
        self
    }

    pub fn with_next<F: Fn(Act) -> Act>(mut self, build: F) -> Self {
        self.next = Some(Box::new(build(Act::default())));
        self
    }

    pub fn with_then(mut self, build: fn(Vec<Act>) -> Vec<Act>) -> Self {
        let stmts = Vec::new();
        self.then = build(stmts);
        self
    }
}

impl From<Block> for Act {
    fn from(val: Block) -> Self {
        Act::block(|_| val.clone())
    }
}
