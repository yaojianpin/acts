use crate::{Act, Catch, Timeout, Vars};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Req {
    #[serde(default)]
    pub id: String,

    #[serde(default)]
    pub name: String,

    #[serde(default)]
    pub key: String,

    #[serde(default)]
    pub tag: String,

    #[serde(default)]
    pub inputs: Vars,

    #[serde(default)]
    pub outputs: Vars,

    #[serde(default)]
    pub rets: Vars,

    #[serde(default)]
    pub on_created: Vec<Act>,
    #[serde(default)]
    pub on_completed: Vec<Act>,
    #[serde(default)]
    pub catches: Vec<Catch>,
    #[serde(default)]
    pub timeout: Vec<Timeout>,
}

impl Req {
    pub fn new() -> Self {
        Default::default()
    }
    pub fn with_id(mut self, id: &str) -> Self {
        self.id = id.to_string();
        self
    }

    pub fn with_name(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    pub fn with_tag(mut self, tag: &str) -> Self {
        self.tag = tag.to_string();
        self
    }

    pub fn with_key(mut self, key: &str) -> Self {
        self.key = key.to_string();
        self
    }

    pub fn with_input<T>(mut self, name: &str, value: T) -> Self
    where
        T: Serialize + Clone,
    {
        self.inputs.set(name, value);
        self
    }

    pub fn with_output<T>(mut self, name: &str, value: T) -> Self
    where
        T: Serialize + Clone,
    {
        self.outputs.set(name, value);
        self
    }

    pub fn with_ret<T>(mut self, name: &str, value: T) -> Self
    where
        T: Serialize + Clone,
    {
        self.rets.set(name, value);
        self
    }

    pub fn with_on_created(mut self, build: fn(Vec<Act>) -> Vec<Act>) -> Self {
        let stmts = Vec::new();
        self.on_created = build(stmts);
        self
    }

    pub fn with_on_completed(mut self, build: fn(Vec<Act>) -> Vec<Act>) -> Self {
        let stmts = Vec::new();
        self.on_completed = build(stmts);
        self
    }

    pub fn with_catch(mut self, build: fn(Catch) -> Catch) -> Self {
        let c = Catch::default();
        self.catches.push(build(c));
        self
    }

    pub fn with_timeout(mut self, build: fn(Timeout) -> Timeout) -> Self {
        let c = Timeout::default();
        self.timeout.push(build(c));
        self
    }
}
