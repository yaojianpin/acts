use super::DbModel;
use acts_tag::{Tags, Value};
use serde::{Deserialize, Serialize};

#[derive(Tags, Deserialize, Serialize, Debug, Clone)]
pub struct Proc {
    #[tag(id)]
    pub id: String,
    #[tag]
    pub pid: String,
    pub model: String,
    pub state: String,
    pub start_time: i64,
    pub end_time: i64,
    pub vars: String,
}

impl DbModel for Proc {
    fn id(&self) -> &str {
        &self.id
    }
}
