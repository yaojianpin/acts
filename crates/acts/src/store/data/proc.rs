use serde::{Deserialize, Serialize};

use crate::store::{DbCollectionIden, StoreIden};

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
    pub env: String,
    pub err: Option<String>,
}

impl DbCollectionIden for Proc {
    fn iden() -> StoreIden {
        StoreIden::Procs
    }
}
