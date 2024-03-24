use crate::{
    store::{
        db::local::{DbColumn, DbRow, DbSchema, DbType},
        Proc,
    },
    Result,
};
use duckdb::{types::Value, Error as DbError, Result as DbResult};

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
            "vars".to_string(),
            DbColumn {
                db_type: DbType::Text,
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
            "root_tid".to_string(),
            DbColumn {
                db_type: DbType::Text,
                is_not_null: true,
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

    fn from_row<'a>(row: &duckdb::Row<'a>) -> DbResult<Proc, DbError> {
        Ok(Proc {
            id: row.get::<usize, String>(0).unwrap(),
            name: row.get::<usize, String>(1).unwrap(),
            state: row.get::<usize, String>(2).unwrap(),
            mid: row.get::<usize, String>(3).unwrap(),
            start_time: row.get::<usize, i64>(4).unwrap(),
            end_time: row.get::<usize, i64>(5).unwrap(),
            vars: row.get::<usize, String>(6).unwrap(),
            timestamp: row.get::<usize, i64>(7).unwrap(),
            model: row.get::<usize, String>(8).unwrap(),
            root_tid: row.get::<usize, String>(9).unwrap(),
        })
    }

    fn to_values(&self) -> Result<Vec<(String, Value)>> {
        let mut ret = Vec::new();

        ret.push(("id".to_string(), Value::Text(self.id.clone())));
        ret.push(("name".to_string(), Value::Text(self.name.clone())));
        ret.push(("state".to_string(), Value::Text(self.state.clone())));
        ret.push(("mid".to_string(), Value::Text(self.mid.clone())));
        ret.push(("start_time".to_string(), Value::BigInt(self.start_time)));
        ret.push(("end_time".to_string(), Value::BigInt(self.end_time)));
        ret.push(("vars".to_string(), Value::Text(self.vars.clone())));
        ret.push(("timestamp".to_string(), Value::BigInt(self.timestamp)));
        ret.push(("model".to_string(), Value::Text(self.model.clone())));
        ret.push(("root_tid".to_string(), Value::Text(self.root_tid.clone())));

        Ok(ret)
    }
}
