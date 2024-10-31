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
            "state".to_string(),
            DbColumn {
                db_type: DbType::Text,
                is_not_null: true,
                is_primary_key: false,
                is_index: true,
                ..Default::default()
            },
        ));
        map.push((
            "mid".to_string(),
            DbColumn {
                db_type: DbType::Text,
                is_not_null: true,
                is_primary_key: false,
                is_index: true,
                ..Default::default()
            },
        ));
        map.push((
            "start_time".to_string(),
            DbColumn {
                db_type: DbType::Int64,
                is_not_null: true,
                is_primary_key: false,
                ..Default::default()
            },
        ));
        map.push((
            "end_time".to_string(),
            DbColumn {
                db_type: DbType::Int64,
                is_not_null: true,
                is_primary_key: false,
                ..Default::default()
            },
        ));
        map.push((
            "timestamp".to_string(),
            DbColumn {
                db_type: DbType::Int64,
                is_not_null: true,
                is_primary_key: false,
                ..Default::default()
            },
        ));
        map.push((
            "model".to_string(),
            DbColumn {
                db_type: DbType::Text,
                ..Default::default()
            },
        ));
        map.push((
            "env_local".to_string(),
            DbColumn {
                db_type: DbType::Text,
                ..Default::default()
            },
        ));

        map.push((
            "err".to_string(),
            DbColumn {
                db_type: DbType::Text,
                ..Default::default()
            },
        ));
        Ok(map)
    }
}

impl DbRow for Proc {
    fn id(&self) -> &str {
        &self.id
    }

    fn from_row<'a>(row: &Row<'a>) -> DbResult<Proc, DbError> {
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
        let mut ret = Vec::new();

        ret.push(("id".to_string(), Value::Text(self.id.clone())));
        ret.push(("name".to_string(), Value::Text(self.name.clone())));
        ret.push(("state".to_string(), Value::Text(self.state.clone())));
        ret.push(("mid".to_string(), Value::Text(self.mid.clone())));
        ret.push(("start_time".to_string(), Value::Integer(self.start_time)));
        ret.push(("end_time".to_string(), Value::Integer(self.end_time)));
        ret.push(("timestamp".to_string(), Value::Integer(self.timestamp)));
        ret.push(("model".to_string(), Value::Text(self.model.clone())));
        ret.push(("env_local".to_string(), Value::Text(self.env_local.clone())));
        ret.push((
            "err".to_string(),
            match &self.err {
                Some(v) => Value::Text(v.clone()),
                None => Value::Null,
            },
        ));
        Ok(ret)
    }
}
