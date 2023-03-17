use crate::{ActValue, ModelBase, ShareLock};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Act {
    #[serde(default)]
    pub id: String,

    #[serde(default)]
    pub owner: String,

    #[serde(skip)]
    pub step_id: String,

    #[serde(skip)]
    pub env: ShareLock<HashMap<String, ActValue>>,

    #[serde(skip)]
    pub user: ShareLock<Option<String>>,
}

impl ModelBase for Act {
    fn id(&self) -> &str {
        &self.id
    }
}
