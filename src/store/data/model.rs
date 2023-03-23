use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Model {
    pub id: String,
    pub model: String,
    pub ver: u32,
}
