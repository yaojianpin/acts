use crate::{TaskState, Vars, data, utils};
use core::fmt;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(
    Serialize,
    Deserialize,
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    strum::AsRefStr,
    strum::EnumString,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
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
    /// message id
    pub id: String,

    /// task id
    pub tid: String,

    /// node name or action name
    pub name: String,

    /// task action state
    pub state: MessageState,

    /// message type
    /// workflow | step | branch | act
    pub r#type: String,

    pub model: Model,

    /// process id
    pub pid: String,

    /// node id
    pub nid: String,

    /// model id
    pub mid: String,

    /// node id or act
    /// if the key is empty, just using nid as the key
    pub key: String,

    /// used package name
    pub uses: String,

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

    /// record the message retry times
    pub retry_times: i32,
}

impl Message {
    pub fn state(&self) -> MessageState {
        self.state
    }

    pub fn is_key(&self, key: &str) -> bool {
        self.key == key
    }

    pub fn is_uses(&self, uses: &str) -> bool {
        self.uses == uses
    }

    pub fn is_irq(&self) -> bool {
        self.uses == "acts.core.irq"
    }

    pub fn is_msg(&self) -> bool {
        self.uses == "acts.core.msg"
    }

    pub fn is_state(&self, state: MessageState) -> bool {
        self.state == state
    }

    pub fn is_type(&self, t: &str) -> bool {
        self.r#type == t
    }

    pub fn is_tag(&self, tag: &str) -> bool {
        self.tag == tag
    }

    pub fn type_of(&self, mtype: &str) -> Option<&Self> {
        if self.r#type == mtype {
            return Some(self);
        }
        None
    }

    pub fn tag_of(&self, tag: &str) -> Option<&Self> {
        if tag == self.tag {
            return Some(self);
        }

        None
    }

    pub fn key_of(&self, key: &str) -> Option<&Self> {
        if key == self.key {
            return Some(self);
        }

        None
    }

    /// workflow cost in million seconds
    pub fn cost(&self) -> i64 {
        if self.state().is_completed() {
            return self.end_time - self.start_time;
        }

        0
    }

    pub fn into(&self, emit_id: &str, pat: &str) -> data::Message {
        let value = self.clone();
        data::Message {
            id: value.id,
            tid: value.tid,
            name: value.name,
            state: value.state,
            r#type: value.r#type,
            model: serde_json::to_string(&value.model).unwrap(),
            pid: value.pid,
            nid: value.nid,
            mid: value.mid,
            key: value.key,
            uses: value.uses,
            inputs: value.inputs.to_string(),
            outputs: value.outputs.to_string(),
            tag: value.tag,
            start_time: value.start_time,
            end_time: value.end_time,
            chan_id: emit_id.to_string(),
            chan_pattern: pat.to_string(),
            create_time: utils::time::time_millis(),
            update_time: 0,
            retry_times: 0,
            timestamp: utils::time::timestamp(),
            status: data::MessageStatus::Created,
        }
    }
}

impl MessageState {
    pub fn is_completed(&self) -> bool {
        matches!(
            self,
            MessageState::Completed
                | MessageState::Cancelled
                | MessageState::Submitted
                | MessageState::Backed
                | MessageState::Error
                | MessageState::Skipped
                | MessageState::Aborted
                | MessageState::Removed
        )
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
            TaskState::Ready | TaskState::Pending | TaskState::Running | TaskState::Interrupt => {
                MessageState::Created
            }
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
        state.as_ref().to_string()
    }
}

impl From<data::Message> for Message {
    fn from(v: data::Message) -> Self {
        Self {
            id: v.id,
            tid: v.tid,
            name: v.name,
            state: v.state,
            r#type: v.r#type,
            model: serde_json::from_str(&v.model).unwrap_or_default(),
            pid: v.pid,
            nid: v.nid,
            mid: v.mid,
            key: v.key,
            uses: v.uses,
            inputs: serde_json::from_str(&v.inputs).unwrap_or_default(),
            outputs: serde_json::from_str(&v.outputs).unwrap_or_default(),
            tag: v.tag,
            start_time: v.start_time,
            end_time: v.end_time,
            retry_times: v.retry_times,
        }
    }
}

impl From<String> for MessageState {
    fn from(str: String) -> Self {
        Self::from_str(str.as_ref()).unwrap_or_default()
    }
}

impl From<&MessageState> for String {
    fn from(state: &MessageState) -> Self {
        state.as_ref().to_string()
    }
}
