use crate::{sch::TaskState, utils, ActError, ActResult, ShareLock, Vars};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, RwLock};

#[derive(Clone, Debug)]
pub struct Message {
    pub id: String,
    pub pid: String,
    pub tid: String,
    pub create_time: i64,
    pub update_time: i64,
    pub uid: Option<String>,
    pub vars: Vars,
    state: ShareLock<TaskState>,
}

#[derive(Clone, Debug)]
pub struct UserMessage {
    // pub id: Option<String>,
    pub pid: String,
    pub uid: String,
    pub action: String,
    pub options: Option<ActionOptions>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ActionOptions {
    pub vars: Vars,
    pub to: Option<String>,
    pub biz_id: Option<String>,
}

impl ActionOptions {
    pub fn from_json(data: &str) -> ActResult<Self> {
        let v: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(err) => return Err(ActError::ConvertError(err.to_string())),
        };

        let vars = match &v["vars"] {
            Value::Object(map) => utils::vars::from_json(map),
            _ => {
                return Err(ActError::ConvertError(
                    "json format error to convert ActionOptions".into(),
                ))
            }
        };
        let to = match &v["to"] {
            Value::Null => None,
            Value::String(to) => Some(to.clone()),
            _ => {
                return Err(ActError::ConvertError(
                    "json format error to convert ActionOptions".into(),
                ))
            }
        };

        let biz_id = match &v["biz_id"] {
            Value::Null => None,
            Value::String(biz_id) => Some(biz_id.clone()),
            _ => {
                return Err(ActError::ConvertError(
                    "json format error to convert ActionOptions".into(),
                ))
            }
        };
        Ok(ActionOptions { vars, to, biz_id })
    }
}

impl Message {
    pub fn new(pid: &str, tid: &str, uid: Option<String>, vars: Vars) -> Self {
        let id = utils::Id::new(pid, tid);
        Self {
            id: id.id(),
            pid: pid.to_string(),
            tid: tid.to_string(),
            uid,
            create_time: utils::time::time(),
            update_time: 0,
            vars,
            state: Arc::new(RwLock::new(TaskState::None)),
        }
    }

    pub fn id(&self) -> String {
        self.id.clone()
    }

    pub fn state(&self) -> TaskState {
        self.state.read().unwrap().clone()
    }

    pub fn set_state(&self, state: TaskState) {
        *self.state.write().unwrap() = state;
    }
}

impl UserMessage {
    pub fn new(pid: &str, uid: &str, action: &str, options: Option<ActionOptions>) -> Self {
        Self {
            pid: pid.to_string(),
            uid: uid.to_string(),
            action: action.to_string(),
            options,
        }
    }
}
