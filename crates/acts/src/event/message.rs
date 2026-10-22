use crate::{
    Vars, data,
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
            status: data::DeliveryStatus::Created,
            retry_times: 0,
            create_time: utils::time::time_millis(),
            update_time: 0,
            timestamp: self.timestamp,
            v: data::Delivery::version(),
        }
    }

    /// Pin a failure on the message: `ecode`/`message` carry it. They go into
    /// the message's `outputs` — what a consumer reads to learn why the run or
    /// act failed — and stay in its `inputs` too, which is where every existing
    /// client (and the catch predicates' own vocabulary) reads them.
    pub fn set_err(&mut self, ecode: &str, message: &str) {
        self.inputs.set(consts::ACT_ERR_CODE, ecode);
        self.inputs.set(consts::ACT_ERR_MESSAGE, message);
        self.outputs.set(consts::ACT_ERR_CODE, ecode);
        self.outputs.set(consts::ACT_ERR_MESSAGE, message);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn message() -> Message {
        Message {
            nid: "step1".to_string(),
            uses: Some("acts.core.msg".to_string()),
            r#type: "act".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn node_and_use_predicates_read_the_message() {
        let msg = message();
        assert!(msg.is_nid("step1"));
        assert!(!msg.is_nid("step2"));
        assert!(msg.is_uses("acts.core.msg"));
        assert!(!msg.is_uses("acts.core.irq"));
        assert!(msg.is_msg());
        assert!(!msg.is_irq());

        let mut irq = message();
        irq.uses = Some("acts.core.irq".to_string());
        assert!(irq.is_irq());
        assert!(!irq.is_msg());

        // `is_uses`/`is_irq` require the act message type.
        let mut typed = message();
        typed.r#type = "step".to_string();
        assert!(!typed.is_uses("acts.core.msg"));
        assert!(!typed.is_msg());
    }

    #[test]
    fn a_tag_is_read_from_the_message_options() {
        let mut msg = message();
        assert!(!msg.is_tag("vip"), "no options means no tag");
        msg.inputs = Vars::new().with("options", Vars::new());
        assert!(!msg.is_tag("vip"), "options without a tag match nothing");
        msg.inputs = Vars::new().with("options", Vars::new().with("tag", "vip"));
        assert!(msg.is_tag("vip"));
        assert!(!msg.is_tag("other"));
    }

    #[test]
    fn state_strings_round_trip_through_the_wire_vocabulary() {
        for state in [
            MessageState::None,
            MessageState::Created,
            MessageState::Completed,
            MessageState::Submitted,
            MessageState::Backed,
            MessageState::Cancelled,
            MessageState::Aborted,
            MessageState::Skipped,
            MessageState::Error,
            MessageState::Removed,
        ] {
            let text: String = state.into();
            assert_eq!(MessageState::from(text.clone()), state);
            assert_eq!(text, state.to_string());
        }

        // an unknown name falls back to the default state
        assert_eq!(
            MessageState::from("no_such_state".to_string()),
            MessageState::None
        );
    }

    #[test]
    fn a_stored_message_rebuilds_without_a_delivery_id() {
        let stored = data::Message {
            id: "m1".to_string(),
            tid: "t1".to_string(),
            name: "n".to_string(),
            state: MessageState::Completed,
            r#type: "act".to_string(),
            pid: "p1".to_string(),
            nid: "n1".to_string(),
            mid: "mid".to_string(),
            uses: Some("acts.core.msg".to_string()),
            inputs: r#"{"a":1}"#.to_string(),
            outputs: "not json".to_string(),
            ..Default::default()
        };
        let msg = Message::from(stored);
        assert_eq!(msg.id, "m1");
        assert_eq!(msg.delivery_id, None);
        assert_eq!(msg.inputs.get::<i32>("a"), Some(1));
        // an unreadable stored payload degrades to empty rather than failing
        assert!(msg.outputs.is_empty());
    }
}
