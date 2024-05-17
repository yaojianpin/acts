use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Proc {
    pub id: String,
    pub state: String,
    pub mid: String,
    pub name: String,
    pub start_time: i64,
    pub end_time: i64,
    pub timestamp: i64,
    pub model: String,
    pub env_local: String,
    pub err: Option<String>,
}
