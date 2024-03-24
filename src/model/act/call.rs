use serde::{Deserialize, Serialize};

use crate::Vars;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Call {
    #[serde(default)]
    pub id: String,

    // model id
    #[serde(default)]
    pub mid: String,

    #[serde(default)]
    pub inputs: Vars,

    #[serde(default)]
    pub outputs: Vars,
}

impl Call {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn with_id(mut self, id: &str) -> Self {
        self.id = id.to_string();
        self
    }

    pub fn with_mid(mut self, mid: &str) -> Self {
        self.mid = mid.to_string();
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
}
