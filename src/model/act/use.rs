use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActUse {
    #[serde(default)]
    pub id: String,
}

impl ActUse {
    pub fn new(id: &str) -> Self {
        Self { id: id.to_string() }
    }
}
