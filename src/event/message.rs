use crate::{utils, Msg, TaskState, Vars};
use core::fmt;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub enum MessageState {
    #[default]
    None,
    Created,
    Completed,
    Submitted,
    Backed,
    Cancelled,
    Aborted,
    Skipped,
    Error,
    Removed,
}

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
    pub state: MessageState,

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

    pub fn state(&self) -> MessageState {
        let state = self.state.clone().into();
        state
    }

    pub fn is_key(&self, key: &str) -> bool {
        self.key == key
    }

    pub fn is_state(&self, state: &str) -> bool {
        self.state == state.into()
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

impl fmt::Display for MessageState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::result::Result<(), std::fmt::Error> {
        let s: String = self.into();
        f.write_str(&s)
    }
}

impl From<TaskState> for MessageState {
    fn from(state: TaskState) -> Self {
        match state {
            TaskState::None => MessageState::None,
            TaskState::Pending | TaskState::Running | TaskState::Interrupt => MessageState::Created,
            TaskState::Completed => MessageState::Completed,
            TaskState::Submitted => MessageState::Submitted,
            TaskState::Backed => MessageState::Backed,
            TaskState::Cancelled => MessageState::Cancelled,
            TaskState::Error => MessageState::Error,
            TaskState::Aborted => MessageState::Aborted,
            TaskState::Skipped => MessageState::Skipped,
            TaskState::Removed => MessageState::Removed,
        }
    }
}

impl From<MessageState> for String {
    fn from(state: MessageState) -> Self {
        utils::message_state_to_str(state)
    }
}

impl From<&str> for MessageState {
    fn from(str: &str) -> Self {
        utils::str_to_message_state(str)
    }
}

impl From<String> for MessageState {
    fn from(str: String) -> Self {
        utils::str_to_message_state(&str)
    }
}

impl From<&MessageState> for String {
    fn from(state: &MessageState) -> Self {
        utils::message_state_to_str(state.clone())
    }
}
