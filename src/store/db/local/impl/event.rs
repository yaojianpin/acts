use crate::{
    Result,
    store::{
        Event,
        db::local::{DbColumn, DbRow, DbSchema, DbType},
    },
};
use rusqlite::{Error as DbError, Result as DbResult, Row, types::Value};
impl DbSchema for Event {
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
                "mid".to_string(),
                DbColumn {
                    db_type: DbType::Text,
                    is_not_null: true,
                    is_index: true,
                    ..Default::default()
                },
            ),
            (
                "create_time".to_string(),
                DbColumn {
                    db_type: DbType::Int64,
                    ..Default::default()
                },
            ),
            (
                "timestamp".to_string(),
                DbColumn {
                    db_type: DbType::Int64,
                    ..Default::default()
                },
            ),
        ];

        Ok(map)
    }
}

impl DbRow for Event {
    fn id(&self) -> &str {
        &self.id
    }

    fn from_row(row: &Row<'_>) -> DbResult<Event, DbError> {
        Ok(Event {
            id: row.get::<usize, String>(0).unwrap(),
            name: row.get::<usize, String>(1).unwrap(),
            mid: row.get::<usize, String>(2).unwrap(),
            create_time: row.get::<usize, i64>(3).unwrap(),
            timestamp: row.get::<usize, i64>(4).unwrap(),
        })
    }

    fn to_values(&self) -> Result<Vec<(String, Value)>> {
        let ret = vec![
            ("id".to_string(), Value::Text(self.id.clone())),
            ("name".to_string(), Value::Text(self.name.clone())),
            ("mid".to_string(), Value::Text(self.mid.clone())),
            ("create_time".to_string(), Value::Integer(self.create_time)),
            ("timestamp".to_string(), Value::Integer(self.timestamp)),
        ];

        Ok(ret)
    }
}
