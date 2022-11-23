use crate::{model::Step, sch::TaskState, ShareLock};
use serde::{Deserialize, Serialize};
use serde_yaml::Value;
use std::collections::HashMap;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Branch {
    #[serde(default)]
    pub name: String,

    #[serde(default)]
    pub id: String,

    #[serde(default)]
    pub env: HashMap<String, Value>,

    #[serde(default)]
    pub accept: Option<Value>,

    #[serde(default)]
    pub run: Option<String>,
    pub uses: Option<String>,

    #[serde(default)]
    pub on: Vec<String>,

    pub r#if: Option<String>,

    #[serde(default)]
    pub steps: Vec<Step>,

    #[serde(default)]
    pub next: Option<String>,

    #[serde(skip)]
    pub(crate) state: ShareLock<TaskState>,
    #[serde(skip)]
    pub(crate) start_time: ShareLock<i64>,
    #[serde(skip)]
    pub(crate) end_time: ShareLock<i64>,
}
