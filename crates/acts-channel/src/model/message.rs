use crate::Vars;
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

// #[derive(Default, Serialize, Deserialize, Clone, Debug)]
// pub struct Model {
//     /// workflow id
//     pub id: String,

//     /// workflow tag
//     pub tag: String,

//     /// workflow name
//     pub name: String,
// }
#[derive(Default, Serialize, Deserialize, Clone, Debug)]
pub struct Message {
    /// message id
    pub id: String,

    /// task id
    pub tid: String,

    /// node id
    pub nid: String,

    /// model id
    pub mid: String,

    /// node name or action name
    pub name: String,

    /// task action state
    pub state: String,

    /// message type
    /// msg | req
    pub r#type: String,

    /// proc id
    pub pid: String,

    /// from the task inputs
    pub inputs: Vars,

    /// set the outputs vars when complete the action
    pub outputs: Vars,

    /// tag to distinguish different message
    /// it is from node tag or group tag
    #[serde(default)]
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
        self.state.as_str().into()
    }

    pub fn is_state(&self, state: &str) -> bool {
        self.state == state
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

    /// workflow cost in million seconds
    pub fn cost(&self) -> i64 {
        let state: MessageState = self.state.as_str().into();
        if state.is_completed() {
            return self.end_time - self.start_time;
        }

        0
    }

    pub fn is_params_key(&self, key: &str) -> bool {
        if let Some(params) = self.inputs.get::<Vars>("params")
            && let Some(k) = params.get::<String>("key")
        {
            return k == key;
        }

        false
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

impl From<&MessageState> for String {
    fn from(state: &MessageState) -> Self {
        message_state_to_str(state)
    }
}

impl From<&str> for MessageState {
    fn from(str: &str) -> Self {
        str_to_message_state(str)
    }
}

impl From<String> for MessageState {
    fn from(str: String) -> Self {
        str_to_message_state(&str)
    }
}

fn message_state_to_str(state: &MessageState) -> String {
    match state {
        MessageState::None => "none".to_string(),
        MessageState::Aborted => "aborted".to_string(),
        MessageState::Backed => "backed".to_string(),
        MessageState::Cancelled => "cancelled".to_string(),
        MessageState::Completed => "completed".to_string(),
        MessageState::Created => "created".to_string(),
        MessageState::Skipped => "skipped".to_string(),
        MessageState::Submitted => "submitted".to_string(),
        MessageState::Error => "error".to_string(),
        MessageState::Removed => "removed".to_string(),
    }
}

fn str_to_message_state(s: &str) -> MessageState {
    match s {
        "aborted" => MessageState::Aborted,
        "backed" => MessageState::Backed,
        "cancelled" => MessageState::Cancelled,
        "completed" => MessageState::Completed,
        "created" => MessageState::Created,
        "skipped" => MessageState::Skipped,
        "submitted" => MessageState::Submitted,
        "error" => MessageState::Error,
        "removed" => MessageState::Removed,
        "none" => MessageState::None,
        _ => MessageState::None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Message, MessageState};
    use crate::Vars;
    use serde_json::json;

    fn message() -> Message {
        Message {
            id: "m1".to_string(),
            tid: "t1".to_string(),
            nid: "n1".to_string(),
            mid: "mid1".to_string(),
            name: "act1".to_string(),
            state: "completed".to_string(),
            r#type: "req".to_string(),
            pid: "p1".to_string(),
            inputs: Vars::new().with("params", Vars::new().with("key", "k1")),
            outputs: Vars::new(),
            tag: "g1".to_string(),
            start_time: 10,
            end_time: 35,
            retry_times: 2,
        }
    }

    #[test]
    fn every_state_maps_to_and_from_its_wire_text() {
        assert_eq!(MessageState::default(), MessageState::None);
        let states = [
            (MessageState::None, "none"),
            (MessageState::Created, "created"),
            (MessageState::Completed, "completed"),
            (MessageState::Submitted, "submitted"),
            (MessageState::Backed, "backed"),
            (MessageState::Cancelled, "cancelled"),
            (MessageState::Aborted, "aborted"),
            (MessageState::Skipped, "skipped"),
            (MessageState::Error, "error"),
            (MessageState::Removed, "removed"),
        ];
        for (state, text) in states {
            assert_eq!(state.to_string(), text);
            assert_eq!(String::from(&state), text);
            assert_eq!(MessageState::from(text), state);
            assert_eq!(MessageState::from(text.to_string()), state);
        }

        // an unknown wire text falls back to the neutral state
        assert_eq!(MessageState::from("unknown"), MessageState::None);
        assert_eq!(
            MessageState::from(String::from("unknown")),
            MessageState::None
        );
    }

    #[test]
    fn terminal_states_are_completed() {
        for state in [
            MessageState::Completed,
            MessageState::Cancelled,
            MessageState::Submitted,
            MessageState::Backed,
            MessageState::Error,
            MessageState::Skipped,
            MessageState::Aborted,
            MessageState::Removed,
        ] {
            assert!(state.is_completed(), "{state:?} must be terminal");
        }
        assert!(!MessageState::None.is_completed());
        assert!(!MessageState::Created.is_completed());
    }

    #[test]
    fn accessors_read_the_wire_fields() {
        let message = message();
        assert_eq!(message.state(), MessageState::Completed);
        assert!(message.is_state("completed"));
        assert!(!message.is_state("created"));
        assert!(message.is_tag("g1"));
        assert!(!message.is_tag("other"));
        assert_eq!(message.type_of("req").map(|m| m.id.as_str()), Some("m1"));
        assert!(message.type_of("msg").is_none());
        assert_eq!(message.tag_of("g1").map(|m| m.id.as_str()), Some("m1"));
        assert!(message.tag_of("other").is_none());
        assert_eq!(message.cost(), 25);
        assert!(message.is_params_key("k1"));
        assert!(!message.is_params_key("other"));
    }

    #[test]
    fn cost_is_only_measured_for_a_completed_message() {
        let created = Message {
            state: "created".to_string(),
            start_time: 1,
            end_time: 100,
            ..Default::default()
        };
        assert_eq!(created.cost(), 0);
        // the default state text is empty, which maps to `None`
        assert_eq!(Message::default().cost(), 0);
    }

    #[test]
    fn is_params_key_needs_a_params_map_with_a_key() {
        assert!(!Message::default().is_params_key("k"));

        let not_a_map = Message {
            inputs: Vars::new().with("params", "text"),
            ..Default::default()
        };
        assert!(!not_a_map.is_params_key("k"));

        let no_key = Message {
            inputs: Vars::new().with("params", Vars::new()),
            ..Default::default()
        };
        assert!(!no_key.is_params_key("k"));

        let other_key = Message {
            inputs: Vars::new().with("params", Vars::new().with("key", "other")),
            ..Default::default()
        };
        assert!(!other_key.is_params_key("k"));
    }

    #[test]
    fn serde_roundtrip_defaults_the_tag() {
        let message = message();
        let text = serde_json::to_string(&message).unwrap();
        let back: Message = serde_json::from_str(&text).unwrap();
        assert_eq!(back.id, "m1");
        assert_eq!(back.tag, "g1");
        assert_eq!(back.retry_times, 2);
        assert_eq!(
            back.inputs
                .get::<Vars>("params")
                .unwrap()
                .get::<String>("key")
                .as_deref(),
            Some("k1")
        );

        // `tag` is defaulted so payloads written before it existed still decode
        let mut value: serde_json::Value = serde_json::from_str(&text).unwrap();
        value.as_object_mut().unwrap().remove("tag");
        let back: Message = serde_json::from_value(value).unwrap();
        assert_eq!(back.tag, "");

        assert_eq!(
            serde_json::to_value(MessageState::Completed).unwrap(),
            json!("Completed")
        );
        assert_eq!(
            serde_json::from_value::<MessageState>(json!("Error")).unwrap(),
            MessageState::Error
        );
    }
}
