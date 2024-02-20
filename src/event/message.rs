use super::ActionState;
use crate::{utils, Msg, Vars};
use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize, Clone, Debug)]
pub struct Model {
    /// workflow id
    pub id: String,

    /// workflow tag
    pub tag: String,

    /// workflow name
    pub name: String,
}
#[derive(Default, Serialize, Deserialize, Clone, Debug)]
pub struct Message {
    /// task id
    pub id: String,

    /// node name or action name
    pub name: String,

    /// task action state
    pub state: String,

    /// message type
    /// msg | req
    pub r#type: String,

    // node kind
    pub source: String,

    pub model: Model,

    /// proc id
    pub proc_id: String,

    /// nodeId or specific message key
    pub key: String,

    /// from the task inputs
    pub inputs: Vars,

    /// set the outputs vars when complete the action
    pub outputs: Vars,

    /// tag to distinguish different message
    /// it is from node tag or group tag
    pub tag: String,

    /// task start time in million second
    pub start_time: i64,

    /// task end time in million second
    pub end_time: i64,
}

impl Message {
    pub fn from(msg: &Msg) -> Self {
        Self {
            name: msg.name.to_string(),
            id: msg.id.to_string(),
            tag: msg.tag.to_string(),
            inputs: msg.inputs.clone(),
            start_time: utils::time::time(),
            ..Default::default()
        }
    }

    pub fn state(&self) -> ActionState {
        let state = self.state.clone().into();
        state
    }

    pub fn is_key(&self, key: &str) -> bool {
        self.key == key
    }

    pub fn is_state(&self, state: &str) -> bool {
        self.state == state
    }

    pub fn is_type(&self, t: &str) -> bool {
        self.r#type == t
    }

    pub fn is_source(&self, t: &str) -> bool {
        self.source == t
    }

    pub fn is_tag(&self, tag: &str) -> bool {
        self.tag == tag
    }

    pub fn type_of(&self, mtype: &str) -> Option<&Self> {
        if &self.r#type == mtype {
            return Some(self);
        }
        None
    }

    pub fn tag_of(&self, tag: &str) -> Option<&Self> {
        if tag == &self.tag {
            return Some(self);
        }

        None
    }

    pub fn key_of(&self, key: &str) -> Option<&Self> {
        if key == &self.key {
            return Some(self);
        }

        None
    }
}
