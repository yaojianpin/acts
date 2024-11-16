use crate::{Act, Vars};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Pack {
    #[serde(default)]
    pub key: String,

    #[serde(default)]
    pub inputs: Vars,

    #[serde(default)]
    pub outputs: Vars,
}

impl Pack {
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

    pub fn with_output<T>(mut self, name: &str, value: T) -> Self
    where
        T: Serialize + Clone,
    {
        self.outputs.set(name, value);
        self
    }

    pub fn with_key(mut self, key: &str) -> Self {
        self.key = key.to_string();
        self
    }
}

impl From<Pack> for Act {
    fn from(val: Pack) -> Self {
        Act::pack(|_| val.clone())
    }
}
