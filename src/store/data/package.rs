use serde::{Deserialize, Serialize};

#[derive(Default, Deserialize, Serialize, Debug, Clone)]
pub struct Package {
    pub id: String,
    pub name: String,
    pub size: u32,
    #[serde(with = "hex")]
    pub data: Vec<u8>,
    pub create_time: i64,
    pub update_time: i64,
    pub timestamp: i64,
}
