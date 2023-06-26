pub mod local;
pub mod sqlite;

pub use local::LocalStore;
pub use sqlite::SqliteStore;

use crate::ActError;
use std::error::Error;
pub fn map_db_err(err: impl Error) -> ActError {
    ActError::Store(err.to_string())
}

pub fn map_opt_err(err: String) -> ActError {
    ActError::Store(err)
}
