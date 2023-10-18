use crate::Vars;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActCatch {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,

    #[serde(default)]
    pub tag: String,

    #[serde(default)]
    pub err: Option<String>,

    #[serde(default)]
    pub inputs: Vars,

    #[serde(default)]
    pub outputs: Vars,
}
