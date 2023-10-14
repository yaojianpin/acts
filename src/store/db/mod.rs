mod local;
mod mem;

pub use local::LocalStore;
pub use mem::MemStore;

use crate::ActError;
use std::error::Error;
pub fn map_db_err(err: impl Error) -> ActError {
    ActError::Store(err.to_string())
}

pub fn map_opt_err(err: String) -> ActError {
    ActError::Store(err)
}
