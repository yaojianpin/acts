use super::ActAlias;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ActFor {
    #[serde(default)]
    pub by: String,
    #[serde(default)]
    pub alias: ActAlias,
    #[serde(default)]
    pub r#in: String,
}
