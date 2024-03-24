use crate::Vars;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Pack {
    #[serde(default)]
    pub id: String,

    #[serde(default)]
    pub name: String,

    #[serde(default)]
    pub uses: String,

    #[serde(default)]
    pub inputs: Vars,

    #[serde(default)]
    pub outputs: Vars,
}

impl Pack {
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

    pub fn with_uses(mut self, pack_id: &str) -> Self {
        self.uses = pack_id.to_string();
        self
    }
}
