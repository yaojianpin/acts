use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
pub struct Message {
    pub id: String,
    pub pid: String,
    pub tid: String,
    pub uid: String,
    pub vars: String,
    pub create_time: i64,
    pub update_time: i64,
    pub state: u8,
}
