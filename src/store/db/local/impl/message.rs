use crate::{
    MessageState, Result,
    store::{
        Message,
        db::local::{DbColumn, DbRow, DbSchema, DbType},
    },
};
use rusqlite::{Error as DbError, Result as DbResult, Row, types::Value};
use std::str::FromStr;
impl DbSchema for Message {
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
                "tid".to_string(),
                DbColumn {
                    db_type: DbType::Text,
                    is_not_null: true,
                    is_index: true,
                    ..Default::default()
                },
            ),
            (
                "state".to_string(),
                DbColumn {
                    db_type: DbType::Text,
                    is_not_null: true,
                    is_primary_key: false,
                    ..Default::default()
                },
            ),
            (
                "type".to_string(),
                DbColumn {
                    db_type: DbType::Text,
                    is_not_null: true,
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
                "pid".to_string(),
                DbColumn {
                    db_type: DbType::Text,
                    is_not_null: true,
                    is_index: true,
                    ..Default::default()
                },
            ),
            (
                "nid".to_string(),
                DbColumn {
                    db_type: DbType::Text,
                    is_not_null: true,
                    is_index: true,
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
                "key".to_string(),
                DbColumn {
                    db_type: DbType::Text,
                    ..Default::default()
                },
            ),
            (
                "uses".to_string(),
                DbColumn {
                    db_type: DbType::Text,
                    is_not_null: true,
                    is_index: true,
                    ..Default::default()
                },
            ),
            (
                "inputs".to_string(),
                DbColumn {
                    db_type: DbType::Text,
                    ..Default::default()
                },
            ),
            (
                "outputs".to_string(),
                DbColumn {
                    db_type: DbType::Text,
                    ..Default::default()
                },
            ),
            (
                "tag".to_string(),
                DbColumn {
                    db_type: DbType::Text,
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
                "chan_id".to_string(),
                DbColumn {
                    db_type: DbType::Text,
                    is_not_null: true,
                    ..Default::default()
                },
            ),
            (
                "chan_pattern".to_string(),
                DbColumn {
                    db_type: DbType::Text,
                    is_not_null: true,
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
                "update_time".to_string(),
                DbColumn {
                    db_type: DbType::Int64,
                    ..Default::default()
                },
            ),
            (
                "status".to_string(),
                DbColumn {
                    db_type: DbType::Int8,
                    is_index: true,
                    ..Default::default()
                },
            ),
            (
                "retry_times".to_string(),
                DbColumn {
                    db_type: DbType::Int32,
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

impl DbRow for Message {
    fn id(&self) -> &str {
        &self.id
    }

    fn from_row(row: &Row<'_>) -> DbResult<Message, DbError> {
        Ok(Message {
            id: row.get::<usize, String>(0).unwrap(),
            name: row.get::<usize, String>(1).unwrap(),
            tid: row.get::<usize, String>(2).unwrap(),
            state: MessageState::from_str(&row.get::<usize, String>(3).unwrap()).unwrap(),
            r#type: row.get::<usize, String>(4).unwrap(),
            model: row.get::<usize, String>(5).unwrap(),
            pid: row.get::<usize, String>(6).unwrap(),
            nid: row.get::<usize, String>(7).unwrap(),
            mid: row.get::<usize, String>(8).unwrap(),
            key: row.get::<usize, String>(9).unwrap(),
            uses: row.get::<usize, String>(10).unwrap(),
            inputs: row.get::<usize, String>(11).unwrap(),
            outputs: row.get::<usize, String>(12).unwrap(),
            tag: row.get::<usize, String>(13).unwrap(),
            start_time: row.get::<usize, i64>(14).unwrap(),
            end_time: row.get::<usize, i64>(15).unwrap(),
            chan_id: row.get::<usize, String>(16).unwrap(),
            chan_pattern: row.get::<usize, String>(17).unwrap(),
            create_time: row.get::<usize, i64>(18).unwrap(),
            update_time: row.get::<usize, i64>(19).unwrap(),
            status: row.get::<usize, i8>(20).unwrap().into(),
            retry_times: row.get::<usize, i32>(21).unwrap(),
            timestamp: row.get::<usize, i64>(22).unwrap(),
        })
    }

    fn to_values(&self) -> Result<Vec<(String, Value)>> {
        let ret = vec![
            ("id".to_string(), Value::Text(self.id.clone())),
            ("name".to_string(), Value::Text(self.name.clone())),
            ("tid".to_string(), Value::Text(self.tid.clone())),
            (
                "state".to_string(),
                Value::Text(self.state.as_ref().to_string()),
            ),
            ("type".to_string(), Value::Text(self.r#type.clone())),
            ("uses".to_string(), Value::Text(self.uses.clone())),
            ("model".to_string(), Value::Text(self.model.clone())),
            ("pid".to_string(), Value::Text(self.pid.clone())),
            ("nid".to_string(), Value::Text(self.nid.clone())),
            ("mid".to_string(), Value::Text(self.mid.clone())),
            ("key".to_string(), Value::Text(self.key.clone())),
            ("inputs".to_string(), Value::Text(self.inputs.clone())),
            ("outputs".to_string(), Value::Text(self.outputs.clone())),
            ("tag".to_string(), Value::Text(self.tag.clone())),
            ("start_time".to_string(), Value::Integer(self.start_time)),
            ("end_time".to_string(), Value::Integer(self.end_time)),
            ("chan_id".to_string(), Value::Text(self.chan_id.clone())),
            (
                "chan_pattern".to_string(),
                Value::Text(self.chan_pattern.clone()),
            ),
            ("create_time".to_string(), Value::Integer(self.create_time)),
            ("update_time".to_string(), Value::Integer(self.update_time)),
            ("status".to_string(), Value::Integer(self.status.into())),
            (
                "retry_times".to_string(),
                Value::Integer(self.retry_times as i64),
            ),
            ("timestamp".to_string(), Value::Integer(self.timestamp)),
        ];

        Ok(ret)
    }
}
