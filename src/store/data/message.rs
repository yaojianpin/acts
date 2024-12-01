use core::fmt;

use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};

#[derive(Default, Debug, Copy, PartialEq, Clone, Serialize_repr, Deserialize_repr)]
#[repr(i8)]
pub enum MessageStatus {
    #[default]
    Created = 0,
    Acked = 1,
    Completed = 2,
    Error = 3,
}

#[derive(Default, Deserialize, Serialize, Debug, Clone)]
pub struct Message {
    pub id: String,
    pub tid: String,
    pub name: String,
    pub state: String,
    pub r#type: String,
    pub source: String,
    pub model: String,
    pub pid: String,
    pub nid: String,
    pub mid: String,
    pub key: String,
    pub inputs: String,
    pub outputs: String,
    pub tag: String,
    pub start_time: i64,
    pub end_time: i64,
    pub chan_id: String,
    pub chan_pattern: String,

    pub create_time: i64,
    pub update_time: i64,
    pub retry_times: i32,
    pub status: MessageStatus,
    pub timestamp: i64,
}

impl fmt::Display for MessageStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            MessageStatus::Created => "created",
            MessageStatus::Acked => "acked",
            MessageStatus::Completed => "completed",
            MessageStatus::Error => "error",
        })
    }
}

impl From<i8> for MessageStatus {
    fn from(value: i8) -> Self {
        match value {
            1 => MessageStatus::Acked,
            2 => MessageStatus::Completed,
            3 => MessageStatus::Error,
            _ => MessageStatus::Created,
        }
    }
}

impl From<MessageStatus> for i8 {
    fn from(val: MessageStatus) -> i8 {
        match val {
            MessageStatus::Created => 0,
            MessageStatus::Acked => 1,
            MessageStatus::Completed => 2,
            MessageStatus::Error => 3,
        }
    }
}

impl From<MessageStatus> for i64 {
    fn from(val: MessageStatus) -> Self {
        match val {
            MessageStatus::Created => 0,
            MessageStatus::Acked => 1,
            MessageStatus::Completed => 2,
            MessageStatus::Error => 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::MessageStatus;

    #[test]
    fn store_data_message_status_to_i8() {
        let created: i8 = MessageStatus::Created.into();
        assert_eq!(created, 0);

        let created: i8 = MessageStatus::Acked.into();
        assert_eq!(created, 1);

        let created: i8 = MessageStatus::Completed.into();
        assert_eq!(created, 2);

        let created: i8 = MessageStatus::Error.into();
        assert_eq!(created, 3);
    }

    #[test]
    fn store_data_i8_to_message_status() {
        let created: MessageStatus = 0.into();
        assert_eq!(created, MessageStatus::Created);

        let created: MessageStatus = 1.into();
        assert_eq!(created, MessageStatus::Acked);

        let created: MessageStatus = 2.into();
        assert_eq!(created, MessageStatus::Completed);

        let created: MessageStatus = 3.into();
        assert_eq!(created, MessageStatus::Error);

        let created: MessageStatus = 100.into();
        assert_eq!(created, MessageStatus::Created);
    }

    #[test]
    fn store_data_message_status_to_string() {
        assert_eq!(MessageStatus::Created.to_string(), "created");
        assert_eq!(MessageStatus::Acked.to_string(), "acked");
        assert_eq!(MessageStatus::Completed.to_string(), "completed");
        assert_eq!(MessageStatus::Error.to_string(), "error");
    }
}
