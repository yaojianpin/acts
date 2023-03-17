use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
pub struct Message {
    pub id: String,
    pub pid: String,
    pub tid: String,
    pub user: String,
    pub vars: String,
    pub create_time: i64,
}
