use crate::{Act, Vars};
use serde::{Deserialize, Serialize};

/// do an action command
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Do {
    /// action name
    #[serde(default)]
    pub key: String,

    #[serde(default)]
    pub inputs: Vars,
}

impl Do {
    pub fn new() -> Self {
        Default::default()
    }
    pub fn with_key(mut self, name: &str) -> Self {
        self.key = name.to_string();
        self
    }

    pub fn with_input<T>(mut self, name: &str, value: T) -> Self
    where
        T: Serialize + Clone,
    {
        self.inputs.set(name, value);
        self
    }
}

impl From<Do> for Act {
    fn from(val: Do) -> Self {
        Act::cmd(|_| val.clone())
    }
}
