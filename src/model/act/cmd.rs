use crate::Vars;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Cmd {
    #[serde(default)]
    pub name: String,

    #[serde(default)]
    pub inputs: Vars,
}

impl Cmd {
    pub fn new() -> Self {
        Default::default()
    }
    pub fn with_name(mut self, name: &str) -> Self {
        self.name = name.to_string();
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
