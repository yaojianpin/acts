mod r#for;

use crate::{ModelBase, Vars};
pub use r#for::ActFor;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActAlias {
    #[serde(default)]
    pub init: Option<String>,
    pub each: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Act {
    #[serde(default)]
    pub id: String,

    #[serde(default)]
    pub name: String,

    #[serde(default)]
    pub tag: String,

    #[serde(default)]
    pub inputs: Vars,

    #[serde(default)]
    pub outputs: Vars,

    #[serde(default)]
    pub r#for: Option<ActFor>,
}

impl ModelBase for Act {
    fn id(&self) -> &str {
        &self.id
    }
}

impl Act {
    pub fn name(&self) -> &str {
        &self.name
    }
}
