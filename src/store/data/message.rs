use super::DbModel;
use acts_tag::{Tags, Value};
use serde::{Deserialize, Serialize};

#[derive(Tags, Debug, Serialize, Deserialize, Clone)]
pub struct Message {
    #[tag(id)]
    pub id: String,
    #[tag]
    pub pid: String,
    #[tag]
    pub key: String,
    pub kind: String,
    pub event: String,
    pub state: String,
    pub start_time: i64,
    pub end_time: i64,
    pub ack: bool,
}

impl DbModel for Message {
    fn id(&self) -> &str {
        &self.id
    }
}
