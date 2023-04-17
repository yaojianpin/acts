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
    pub tid: String,
    #[tag]
    pub uid: String,
    pub vars: String,
    pub create_time: i64,
    pub update_time: i64,
    pub state: u8,
}

impl DbModel for Message {
    fn id(&self) -> &str {
        &self.id
    }
}
