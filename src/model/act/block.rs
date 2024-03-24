use crate::{Act, Vars};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Block {
    #[serde(default)]
    pub id: String,

    #[serde(default)]
    pub acts: Vec<Act>,

    #[serde(default)]
    pub inputs: Vars,

    #[serde(default)]
    pub next: Option<Box<Block>>,
}

impl Block {
    pub fn new() -> Self {
        Default::default()
    }
    pub fn with_id(mut self, id: &str) -> Self {
        self.id = id.to_string();
        self
    }

    pub fn with_input<T>(mut self, name: &str, value: T) -> Self
    where
        T: Serialize + Clone,
    {
        self.inputs.set(name, value);
        self
    }

    pub fn with_next(mut self, build: fn(Block) -> Block) -> Self {
        let stmt = Block::new();
        self.next = Some(Box::new(build(stmt)));
        self
    }

    pub fn with_acts(mut self, build: fn(Vec<Act>) -> Vec<Act>) -> Self {
        let stmts = Vec::new();
        self.acts = build(stmts);
        self
    }
}
