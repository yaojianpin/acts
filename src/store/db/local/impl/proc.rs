use crate::{
    store::{
        db::local::{DbColumn, DbRow, DbSchema, DbType},
        Proc,
    },
    Result,
};
use rusqlite::{types::Value, Error as DbError, Result as DbResult, Row};

impl DbSchema for Proc {
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
                "state".to_string(),
                DbColumn {
                    db_type: DbType::Text,
                    is_not_null: true,
                    is_primary_key: false,
                    is_index: true,
                    ..Default::default()
                },
            ),
            (
                "mid".to_string(),
                DbColumn {
                    db_type: DbType::Text,
                    is_not_null: true,
                    is_primary_key: false,
                    is_index: true,
                    ..Default::default()
                },
            ),
            (
                "start_time".to_string(),
                DbColumn {
                    db_type: DbType::Int64,
                    is_not_null: true,
                    is_primary_key: false,
                    ..Default::default()
                },
            ),
            (
                "end_time".to_string(),
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
            (
                "model".to_string(),
                DbColumn {
                    db_type: DbType::Text,
                    ..Default::default()
                },
            ),
            (
                "env_local".to_string(),
                DbColumn {
                    db_type: DbType::Text,
                    ..Default::default()
                },
            ),
            (
                "err".to_string(),
                DbColumn {
                    db_type: DbType::Text,
                    ..Default::default()
                },
            ),
        ];

        Ok(map)
    }
}

impl DbRow for Proc {
    fn id(&self) -> &str {
        &self.id
    }

    fn from_row(row: &Row<'_>) -> DbResult<Proc, DbError> {
        Ok(Proc {
            id: row.get::<usize, String>(0).unwrap(),
            name: row.get::<usize, String>(1).unwrap(),
            state: row.get::<usize, String>(2).unwrap(),
            mid: row.get::<usize, String>(3).unwrap(),
            start_time: row.get::<usize, i64>(4).unwrap(),
            end_time: row.get::<usize, i64>(5).unwrap(),
            timestamp: row.get::<usize, i64>(6).unwrap(),
            model: row.get::<usize, String>(7).unwrap(),
            env_local: row.get::<usize, String>(8).unwrap(),
            err: row.get::<usize, Option<String>>(9).unwrap(),
        })
    }

    fn to_values(&self) -> Result<Vec<(String, Value)>> {
        let ret = vec![
            ("id".to_string(), Value::Text(self.id.clone())),
            ("name".to_string(), Value::Text(self.name.clone())),
            ("state".to_string(), Value::Text(self.state.clone())),
            ("mid".to_string(), Value::Text(self.mid.clone())),
            ("start_time".to_string(), Value::Integer(self.start_time)),
            ("end_time".to_string(), Value::Integer(self.end_time)),
            ("timestamp".to_string(), Value::Integer(self.timestamp)),
            ("model".to_string(), Value::Text(self.model.clone())),
            ("env_local".to_string(), Value::Text(self.env_local.clone())),
            (
                "err".to_string(),
                match &self.err {
                    Some(v) => Value::Text(v.clone()),
                    None => Value::Null,
                },
            ),
        ];
        Ok(ret)
    }
}
