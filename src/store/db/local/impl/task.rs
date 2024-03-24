use crate::{
    store::{
        db::local::{DbColumn, DbRow, DbSchema, DbType},
        Task,
    },
    Result,
};
use duckdb::{types::Value, Error as DbError, Result as DbResult};

impl DbSchema for Task {
    fn schema() -> Result<Vec<(String, DbColumn)>> {
        let mut map = Vec::new();
        map.push((
            "id".to_string(),
            DbColumn {
                db_type: DbType::Text,
                is_not_null: true,
                is_primary_key: true,
                ..Default::default()
            },
        ));
        map.push((
            "name".to_string(),
            DbColumn {
                db_type: DbType::Text,
                ..Default::default()
            },
        ));
        map.push((
            "proc_id".to_string(),
            DbColumn {
                db_type: DbType::Text,
                is_index: true,
                ..Default::default()
            },
        ));
        map.push((
            "task_id".to_string(),
            DbColumn {
                db_type: DbType::Text,
                is_not_null: true,
                ..Default::default()
            },
        ));
        map.push((
            "node_id".to_string(),
            DbColumn {
                db_type: DbType::Text,
                is_not_null: true,
                ..Default::default()
            },
        ));
        map.push((
            "kind".to_string(),
            DbColumn {
                db_type: DbType::Text,
                is_not_null: true,
                ..Default::default()
            },
        ));
        map.push((
            "prev".to_string(),
            DbColumn {
                db_type: DbType::Text,
                ..Default::default()
            },
        ));
        map.push((
            "state".to_string(),
            DbColumn {
                db_type: DbType::Text,
                is_not_null: true,
                ..Default::default()
            },
        ));
        map.push((
            "action_state".to_string(),
            DbColumn {
                db_type: DbType::Text,
                is_not_null: true,
                ..Default::default()
            },
        ));
        map.push((
            "start_time".to_string(),
            DbColumn {
                db_type: DbType::Int64,
                ..Default::default()
            },
        ));
        map.push((
            "end_time".to_string(),
            DbColumn {
                db_type: DbType::Int64,
                ..Default::default()
            },
        ));
        map.push((
            "hooks".to_string(),
            DbColumn {
                db_type: DbType::Text,
                ..Default::default()
            },
        ));
        map.push((
            "timestamp".to_string(),
            DbColumn {
                db_type: DbType::Int64,
                ..Default::default()
            },
        ));
        Ok(map)
    }
}

impl DbRow for Task {
    fn id(&self) -> &str {
        &self.id
    }

    fn from_row<'a>(row: &duckdb::Row<'a>) -> DbResult<Task, DbError> {
        Ok(Task {
            id: row.get::<usize, String>(0).unwrap(),
            name: row.get::<usize, String>(1).unwrap(),
            proc_id: row.get::<usize, String>(2).unwrap(),
            task_id: row.get::<usize, String>(3).unwrap(),
            node_id: row.get::<usize, String>(4).unwrap(),
            kind: row.get::<usize, String>(5).unwrap(),
            prev: row.get::<usize, Option<String>>(6).unwrap(),
            state: row.get::<usize, String>(7).unwrap(),
            action_state: row.get::<usize, String>(8).unwrap(),
            start_time: row.get::<usize, i64>(9).unwrap(),
            end_time: row.get::<usize, i64>(10).unwrap(),
            hooks: row.get::<usize, String>(11).unwrap(),
            timestamp: row.get::<usize, i64>(12).unwrap(),
        })
    }

    fn to_values(&self) -> Result<Vec<(String, Value)>> {
        let mut ret = Vec::new();

        ret.push(("id".to_string(), Value::Text(self.id.clone())));
        ret.push(("name".to_string(), Value::Text(self.name.clone())));
        ret.push(("proc_id".to_string(), Value::Text(self.proc_id.clone())));
        ret.push(("task_id".to_string(), Value::Text(self.task_id.clone())));
        ret.push(("node_id".to_string(), Value::Text(self.node_id.clone())));
        ret.push(("kind".to_string(), Value::Text(self.kind.clone())));
        ret.push((
            "prev".to_string(),
            match &self.prev {
                Some(v) => Value::Text(v.clone()),
                None => Value::Null,
            },
        ));

        ret.push(("state".to_string(), Value::Text(self.state.clone())));
        ret.push((
            "action_state".to_string(),
            Value::Text(self.action_state.clone()),
        ));
        ret.push(("start_time".to_string(), Value::BigInt(self.start_time)));
        ret.push(("end_time".to_string(), Value::BigInt(self.end_time)));
        ret.push(("hooks".to_string(), Value::Text(self.hooks.clone())));
        ret.push(("timestamp".to_string(), Value::BigInt(self.timestamp)));

        Ok(ret)
    }
}
