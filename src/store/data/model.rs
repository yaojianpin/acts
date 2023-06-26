use super::DbModel;
use acts_tag::{Tags, Value};
use serde::{Deserialize, Serialize};

#[derive(Tags, Deserialize, Serialize, Debug, Clone)]
pub struct Model {
    #[tag(id)]
    pub id: String,
    pub name: String,
    pub ver: u32,
    pub size: u32,
    pub time: i64,
    pub model: String,
    pub topic: String,
}

impl DbModel for Model {
    fn id(&self) -> &str {
        &self.id
    }
}
