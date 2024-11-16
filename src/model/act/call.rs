use crate::{Act, Vars};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Call {
    #[serde(default)]
    pub key: String,

    #[serde(default)]
    pub inputs: Vars,

    #[serde(default)]
    pub rets: Vars,
}

impl Call {
    pub fn new() -> Self {
        Default::default()
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

    pub fn with_ret<T>(mut self, name: &str, value: T) -> Self
    where
        T: Serialize + Clone,
    {
        self.rets.set(name, value);
        self
    }
}

impl From<Call> for Act {
    fn from(val: Call) -> Self {
        Act::call(|_| val.clone())
    }
}
