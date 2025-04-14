use crate::{
    store::{
        db::local::{DbColumn, DbRow, DbSchema, DbType},
        Task,
    },
    Result,
};
use rusqlite::{types::Value, Error as DbError, Result as DbResult, Row};

impl DbSchema for Task {
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
                "pid".to_string(),
                DbColumn {
                    db_type: DbType::Text,
                    is_index: true,
                    ..Default::default()
                },
            ),
            (
                "tid".to_string(),
                DbColumn {
                    db_type: DbType::Text,
                    is_not_null: true,
                    ..Default::default()
                },
            ),
            (
                "node_data".to_string(),
                DbColumn {
                    db_type: DbType::Text,
                    is_not_null: true,
                    ..Default::default()
                },
            ),
            (
                "kind".to_string(),
                DbColumn {
                    db_type: DbType::Text,
                    is_not_null: true,
                    ..Default::default()
                },
            ),
            (
                "prev".to_string(),
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
                    ..Default::default()
                },
            ),
            (
                "start_time".to_string(),
                DbColumn {
                    db_type: DbType::Int64,
                    ..Default::default()
                },
            ),
            (
                "end_time".to_string(),
                DbColumn {
                    db_type: DbType::Int64,
                    ..Default::default()
                },
            ),
            (
                "hooks".to_string(),
                DbColumn {
                    db_type: DbType::Text,
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
            (
                "data".to_string(),
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

impl DbRow for Task {
    fn id(&self) -> &str {
        &self.id
    }

    fn from_row(row: &Row<'_>) -> DbResult<Task, DbError> {
        Ok(Task {
            id: row.get::<usize, String>(0).unwrap(),
            name: row.get::<usize, String>(1).unwrap(),
            pid: row.get::<usize, String>(2).unwrap(),
            tid: row.get::<usize, String>(3).unwrap(),
            node_data: row.get::<usize, String>(4).unwrap(),
            kind: row.get::<usize, String>(5).unwrap(),
            prev: row.get::<usize, Option<String>>(6).unwrap(),
            state: row.get::<usize, String>(7).unwrap(),
            start_time: row.get::<usize, i64>(8).unwrap(),
            end_time: row.get::<usize, i64>(9).unwrap(),
            hooks: row.get::<usize, String>(10).unwrap(),
            timestamp: row.get::<usize, i64>(11).unwrap(),
            data: row.get::<usize, String>(12).unwrap(),
            err: row.get::<usize, Option<String>>(13).unwrap(),
        })
    }

    fn to_values(&self) -> Result<Vec<(String, Value)>> {
        let ret = vec![
            ("id".to_string(), Value::Text(self.id.clone())),
            ("name".to_string(), Value::Text(self.name.clone())),
            ("pid".to_string(), Value::Text(self.pid.clone())),
            ("tid".to_string(), Value::Text(self.tid.clone())),
            ("node_data".to_string(), Value::Text(self.node_data.clone())),
            ("kind".to_string(), Value::Text(self.kind.clone())),
            (
                "prev".to_string(),
                match &self.prev {
                    Some(v) => Value::Text(v.clone()),
                    None => Value::Null,
                },
            ),
            ("state".to_string(), Value::Text(self.state.clone())),
            ("start_time".to_string(), Value::Integer(self.start_time)),
            ("end_time".to_string(), Value::Integer(self.end_time)),
            ("hooks".to_string(), Value::Text(self.hooks.clone())),
            ("timestamp".to_string(), Value::Integer(self.timestamp)),
            ("data".to_string(), Value::Text(self.data.clone())),
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
