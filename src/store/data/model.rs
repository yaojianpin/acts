use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub ver: u32,
    pub size: u32,
    pub create_time: i64,
    pub update_time: i64,
    pub data: String,
    pub timestamp: i64,
}
