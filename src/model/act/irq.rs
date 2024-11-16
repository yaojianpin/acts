use crate::{Act, Vars};
use serde::{Deserialize, Serialize};

/// Interrupt request
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Irq {
    #[serde(default)]
    pub key: String,

    #[serde(default)]
    pub tag: String,

    #[serde(default)]
    pub inputs: Vars,

    #[serde(default)]
    pub rets: Vars,

    #[serde(default)]
    pub outputs: Vars,
}

impl Irq {
    pub fn new() -> Self {
        Default::default()
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
}

impl From<Irq> for Act {
    fn from(val: Irq) -> Self {
        Act::irq(|_| val.clone())
    }
}
