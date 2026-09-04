use crate::{
    TaskState, Vars, data,
    store::DbCollectionIden,
    utils::{self, consts},
};
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
pub struct Message {
    /// message id
    pub id: String,

    /// delivery id — set when the message is stored for an ack channel;
    /// identifies this channel's delivery of the message. Ack/retry/clear/
    /// redo operate on it. `None` for broadcasts and non-ack channels.
    #[serde(default)]
    pub delivery_id: Option<String>,

    /// task id
    pub tid: String,

    /// node name or action name
    pub name: String,

    /// task action state
    pub state: MessageState,

    /// message type
    /// workflow | step | branch | act
    pub r#type: String,

    /// process id
    pub pid: String,

    /// node id
    pub nid: String,

    /// model id
    pub mid: String,

    /// used package name
    pub uses: Option<String>,

    /// from the task inputs
    pub inputs: Vars,

    /// set the outputs vars when complete the action
    pub outputs: Vars,

    /// task start time in million second
    pub start_time: i64,

    /// task end time in million second
    pub end_time: i64,

    /// record the message retry times
    pub retry_times: i32,

    /// message timestamp in microseconds
    pub timestamp: i64,
}

impl Message {
    pub fn state(&self) -> MessageState {
        self.state
    }

    pub fn is_nid(&self, nid: &str) -> bool {
        self.nid == nid
    }

    pub fn is_uses(&self, uses: &str) -> bool {
        self.uses.as_deref() == Some(uses) && self.r#type == "act"
    }

    pub fn is_irq(&self) -> bool {
        self.uses.as_deref() == Some("acts.core.irq") && self.r#type == "act"
    }

    pub fn is_msg(&self) -> bool {
        self.uses.as_deref() == Some("acts.core.msg") && self.r#type == "act"
    }

    pub fn is_state(&self, state: MessageState) -> bool {
        self.state == state
    }

    pub fn is_type(&self, t: &str) -> bool {
        self.r#type == t
    }

    pub fn is_tag(&self, tag: &str) -> bool {
        if let Some(options) = self.options()
            && let Some(option_tag) = options.get::<String>("tag")
        {
            return option_tag == tag;
        }
        false
    }

    /// workflow cost in million seconds
    pub fn cost(&self) -> i64 {
        if self.state().is_completed() {
            return self.end_time - self.start_time;
        }

        0
    }

    pub fn params(&self) -> Option<Vars> {
        self.inputs.get::<Vars>("params")
    }

    pub fn options(&self) -> Option<Vars> {
        self.inputs.get::<Vars>("options")
    }

    pub fn is_params_key(&self, key: &str) -> bool {
        if let Some(params) = self.inputs.get::<Vars>("params")
            && let Some(k) = params.get::<String>("key")
        {
            return k == key;
        }

        false
    }

    /// The canonical stored record of the emitted message — the payload and
    /// workflow context are stored once per message id; delivery state is
    /// tracked separately in the `deliveries` collection.
    pub fn into_message(&self) -> data::Message {
        data::Message {
            id: self.id.clone(),
            tid: self.tid.clone(),
            name: self.name.clone(),
            state: self.state,
            r#type: self.r#type.clone(),
            pid: self.pid.clone(),
            nid: self.nid.clone(),
            mid: self.mid.clone(),
            uses: self.uses.clone(),
            inputs: self.inputs.to_string(),
            outputs: self.outputs.to_string(),
            start_time: self.start_time,
            end_time: self.end_time,
            create_time: utils::time::time_millis(),
            timestamp: self.timestamp,
            v: data::Message::version(),
        }
    }

    /// The delivery row of this message to one channel. The delivery gets its
    /// own delivery id; the message id is shared by every channel delivery.
    pub fn into_delivery(&self, chan_id: &str, pattern: &str) -> data::Delivery {
        data::Delivery {
            id: utils::longid(),
            msg_id: self.id.clone(),
            pid: self.pid.clone(),
            tid: self.tid.clone(),
            chan_id: chan_id.to_string(),
            chan_pattern: pattern.to_string(),
            status: data::MessageStatus::Created,
            retry_times: 0,
            create_time: utils::time::time_millis(),
            update_time: 0,
            timestamp: self.timestamp,
            v: data::Delivery::version(),
        }
    }

    pub fn set_err(&mut self, ecode: &str, message: &str) {
        self.inputs.set(consts::ACT_ERR_CODE, ecode);
        self.inputs.set(consts::ACT_ERR_MESSAGE, message);
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

/// Rebuild the emitted message from its canonical stored record. The event
/// carries no delivery id — the redelivery paths tag it afterwards.
impl From<data::Message> for Message {
    fn from(v: data::Message) -> Self {
        Self {
            id: v.id,
            delivery_id: None,
            tid: v.tid,
            name: v.name,
            state: v.state,
            r#type: v.r#type,
            pid: v.pid,
            nid: v.nid,
            mid: v.mid,
            uses: v.uses,
            inputs: serde_json::from_str(&v.inputs).unwrap_or_default(),
            outputs: serde_json::from_str(&v.outputs).unwrap_or_default(),
            start_time: v.start_time,
            end_time: v.end_time,
            retry_times: 0,
            timestamp: v.timestamp,
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
