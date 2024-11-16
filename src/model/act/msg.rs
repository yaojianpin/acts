use crate::{Act, Vars};
use serde::{Deserialize, Serialize};

/// Message struct
/// used to interact with client using grpc
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Msg {
    #[serde(default)]
    pub tag: String,

    #[serde(default)]
    pub key: String,

    #[serde(default)]
    pub inputs: Vars,
}

impl Msg {
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
}

impl From<Msg> for Act {
    fn from(val: Msg) -> Self {
        Act::msg(|_| val.clone())
    }
}
