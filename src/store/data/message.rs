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
    pub key: String,
    pub inputs: String,
    pub outputs: String,
    pub tag: String,
    pub start_time: i64,
    pub end_time: i64,
    pub emit_id: String,
    pub emit_pattern: String,
    pub emit_count: i64,

    pub create_time: i64,
    pub update_time: i64,
    pub retry_times: i32,
    pub status: MessageStatus,
}

impl From<i8> for MessageStatus {
    fn from(value: i8) -> Self {
        match value {
            1 => MessageStatus::Acked,
            2 => MessageStatus::Completed,
            3 => MessageStatus::Error,
            0 | _ => MessageStatus::Created,
        }
    }
}

impl Into<i8> for MessageStatus {
    fn into(self) -> i8 {
        match self {
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
}
