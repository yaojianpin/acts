use super::DbModel;
use acts_tag::{Tags, Value};
use serde::{Deserialize, Serialize};

#[derive(Tags, Debug, Serialize, Deserialize, Clone)]
pub struct Act {
    #[tag(id)]
    pub id: String,
    #[tag]
    pub pid: String,
    #[tag]
    pub tid: String,
    pub vars: String,
    pub kind: String,
    pub event: String,
    pub start_time: i64,
    pub end_time: i64,
    pub state: String,
    pub active: bool,
}

impl DbModel for Act {
    fn id(&self) -> &str {
        &self.id
    }
}
