mod model;
mod proc;
mod task;

use super::db::map_db_err;
use crate::{ActError, Result};
use acts_tag::{Tags, Value};
use serde::{de::DeserializeOwned, Serialize};

pub use model::Model;
pub use proc::Proc;
pub use task::Task;

pub trait DbModel: Tags + Serialize + DeserializeOwned {
    fn id(&self) -> &str;
    fn from_slice(data: &[u8]) -> Result<Self> {
        let value = Value::new(data);
        let value = value.real().map_err(map_db_err)?;
        Ok(value)
    }
    fn to_vec(&self) -> Result<Vec<u8>> {
        let value = Value::from(self).map_err(map_db_err)?;
        Ok(value.data().to_vec())
    }

    fn get(&self, name: &str) -> Result<Vec<u8>> {
        match self.value(name) {
            Some(v) => Ok(v.data().to_vec()),
            None => Err(ActError::Store(format!("fail to get model key '{name}'"))),
        }
    }
}
