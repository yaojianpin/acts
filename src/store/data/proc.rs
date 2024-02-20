use super::DbModel;
use acts_tag::{Tags, Value};
use serde::{Deserialize, Serialize};

#[derive(Tags, Deserialize, Serialize, Debug, Clone)]
pub struct Proc {
    #[tag(id)]
    pub id: String,
    #[tag]
    pub state: String,
    #[tag]
    pub mid: String,
    pub name: String,
    pub start_time: i64,
    pub end_time: i64,
    pub vars: String,
    pub timestamp: i64,
    pub model: String,
    pub root_tid: String,
}

impl DbModel for Proc {
    fn id(&self) -> &str {
        &self.id
    }
}
