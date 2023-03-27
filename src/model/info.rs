use serde::{Deserialize, Serialize};

use crate::{sch::TaskState, store::Model, ActError, ActResult, Vars, Workflow};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ProcInfo {
    pub pid: String,
    pub name: String,
    pub model_id: String,
    pub state: TaskState,
    pub start_time: i64,
    pub end_time: i64,
    pub vars: Vars,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub ver: u32,
    pub size: u32,
    pub time: i64,
    pub model: String,
}

impl ModelInfo {
    pub fn workflow(&self) -> ActResult<Workflow> {
        let m = serde_yaml::from_str(&self.model);
        match m {
            Ok(m) => Ok(m),
            Err(err) => Err(ActError::ConvertError(err.to_string())),
        }
    }
}

impl From<Model> for ModelInfo {
    fn from(m: Model) -> Self {
        Self {
            id: m.id,
            name: m.name,
            ver: m.ver,
            size: m.size,
            time: m.time,

            model: m.model,
        }
    }
}
