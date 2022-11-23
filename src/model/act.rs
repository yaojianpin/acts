use crate::{sch::TaskState, ActValue, ShareLock};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Deserialize, Serialize)]
pub struct Act {
    #[serde(default)]
    pub id: String,

    #[serde(default)]
    pub owner: String,

    #[serde(skip)]
    pub step_task_id: String,

    #[serde(skip)]
    pub env: ShareLock<HashMap<String, ActValue>>,

    #[serde(skip)]
    pub user: ShareLock<Option<String>>,

    #[serde(skip)]
    pub(crate) state: ShareLock<TaskState>,
    #[serde(skip)]
    pub(crate) start_time: ShareLock<i64>,
    #[serde(skip)]
    pub(crate) end_time: ShareLock<i64>,
}
