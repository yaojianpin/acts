use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Proc {
    pub id: String,
    pub pid: String,
    pub model: String,
    pub state: String,
    pub start_time: i64,
    pub end_time: i64,
    pub vars: String,
}
