mod collect;
mod database;
mod r#impl;
mod local;

#[allow(dead_code)]
#[derive(Debug)]
pub enum DbType {
    Boolean,
    Double,
    Decimal(u32, u32),
    Int8,
    Int16,
    Int32,
    Int64,
    Text,
    Binary,
}

#[derive(Debug)]
pub struct DbColumn {
    pub db_type: DbType,
    pub is_not_null: bool,
    pub is_primary_key: bool,
    pub is_unique: bool,
    pub is_index: bool,
    pub default: Option<String>,
}

impl Default for DbColumn {
    fn default() -> Self {
        Self {
            db_type: DbType::Text,
            is_not_null: false,
            is_primary_key: false,
            is_unique: false,
            is_index: false,
            default: None,
        }
    }
}

use crate::Result;
use rusqlite::{types::Value, Error as DbError, Result as DbResult, Row};

trait DbSchema {
    fn schema() -> Result<Vec<(String, DbColumn)>>;
}

trait DbRow {
    fn id(&self) -> &str;
    fn from_row(row: &Row<'_>) -> DbResult<Self, DbError>
    where
        Self: Sized;
    fn to_values(&self) -> Result<Vec<(String, Value)>>;
}

pub use local::LocalStore;
