use crate::{
    store::{
        db::local::{DbColumn, DbRow, DbSchema, DbType},
        Package,
    },
    Result,
};
use rusqlite::{types::Value, Error as DbError, Result as DbResult, Row};
impl DbSchema for Package {
    fn schema() -> Result<Vec<(String, DbColumn)>> {
        let map = vec![
            (
                "id".to_string(),
                DbColumn {
                    db_type: DbType::Text,
                    is_not_null: true,
                    is_primary_key: true,
                    ..Default::default()
                },
            ),
            (
                "name".to_string(),
                DbColumn {
                    db_type: DbType::Text,
                    ..Default::default()
                },
            ),
            (
                "size".to_string(),
                DbColumn {
                    db_type: DbType::Int64,
                    is_not_null: true,
                    is_primary_key: false,
                    ..Default::default()
                },
            ),
            (
                "data".to_string(),
                DbColumn {
                    db_type: DbType::Binary,
                    is_not_null: true,
                    is_primary_key: false,
                    ..Default::default()
                },
            ),
            (
                "create_time".to_string(),
                DbColumn {
                    db_type: DbType::Int64,
                    is_not_null: true,
                    is_primary_key: false,
                    ..Default::default()
                },
            ),
            (
                "update_time".to_string(),
                DbColumn {
                    db_type: DbType::Int64,
                    is_not_null: true,
                    is_primary_key: false,
                    ..Default::default()
                },
            ),
            (
                "timestamp".to_string(),
                DbColumn {
                    db_type: DbType::Int64,
                    is_not_null: true,
                    is_primary_key: false,
                    ..Default::default()
                },
            ),
        ];

        Ok(map)
    }
}

impl DbRow for Package {
    fn id(&self) -> &str {
        &self.id
    }

    fn from_row(row: &Row<'_>) -> DbResult<Package, DbError> {
        Ok(Package {
            id: row.get::<usize, String>(0).unwrap(),
            name: row.get::<usize, String>(1).unwrap(),
            size: row.get::<usize, u32>(2).unwrap(),
            data: row.get::<usize, Vec<u8>>(3).unwrap(),
            create_time: row.get::<usize, i64>(4).unwrap(),
            update_time: row.get::<usize, i64>(5).unwrap(),
            timestamp: row.get::<usize, i64>(6).unwrap(),
        })
    }

    fn to_values(&self) -> Result<Vec<(String, Value)>> {
        let ret = vec![
            ("id".to_string(), Value::Text(self.id.clone())),
            ("name".to_string(), Value::Text(self.name.clone())),
            ("size".to_string(), Value::Integer(self.size as i64)),
            ("data".to_string(), Value::Blob(self.data.clone())),
            ("create_time".to_string(), Value::Integer(self.create_time)),
            ("update_time".to_string(), Value::Integer(self.update_time)),
            ("timestamp".to_string(), Value::Integer(self.timestamp)),
        ];

        Ok(ret)
    }
}
