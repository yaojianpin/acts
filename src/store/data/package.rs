use crate::{
    ActRunAs,
    package::ActPackageCatalog,
    store::{DbCollectionIden, StoreIden},
};
use serde::{Deserialize, Serialize};

#[derive(Default, Deserialize, Serialize, Debug, Clone)]
pub struct Package {
    pub id: String,
    pub desc: String,
    pub icon: String,
    pub doc: String,
    pub version: String,
    pub schema: String,
    pub run_as: ActRunAs,
    pub groups: String,
    pub catalog: ActPackageCatalog,
    pub built_in: bool,

    pub create_time: i64,
    pub update_time: i64,
    pub timestamp: i64,
}

impl DbCollectionIden for Package {
    fn iden() -> StoreIden {
        StoreIden::Packages
    }
}
