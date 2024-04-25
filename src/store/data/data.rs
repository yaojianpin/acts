use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Data {
    pub id: String,
    pub data: String,
}
