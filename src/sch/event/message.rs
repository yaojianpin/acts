use crate::{utils, ActError, ActResult, Vars};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug)]
pub enum MessageState {
    None = 0,
    Sent = 1,
    Received = 2,
}

#[derive(Clone, Debug)]
pub struct Message {
    pub id: String,
    pub pid: String,
    pub tid: String,
    pub create_time: i64,
    pub update_time: i64,
    pub uid: Option<String>,
    pub vars: Vars,
    pub state: MessageState,
}

#[derive(Clone, Debug)]
pub struct UserMessage {
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
            state: MessageState::None,
        }
    }

    pub fn id(&self) -> String {
        self.id.clone()
    }

    pub fn state(&self) -> MessageState {
        self.state.clone()
    }

    pub fn set_state(&mut self, state: MessageState) {
        self.state = state;
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

impl From<crate::store::Message> for Message {
    fn from(m: crate::store::Message) -> Self {
        Message {
            id: m.id,
            pid: m.pid,
            tid: m.tid,
            create_time: m.create_time,
            update_time: m.update_time,
            uid: if m.uid.is_empty() { None } else { Some(m.uid) },
            vars: utils::vars::from_string(&m.vars),
            state: m.state.into(),
        }
    }
}

impl From<u8> for MessageState {
    fn from(v: u8) -> Self {
        match v {
            1 => MessageState::Sent,
            2 => MessageState::Received,
            _ => MessageState::None,
        }
    }
}

impl Into<u8> for MessageState {
    fn into(self) -> u8 {
        match self {
            MessageState::None => 0,
            MessageState::Sent => 1,
            MessageState::Received => 2,
        }
    }
}
