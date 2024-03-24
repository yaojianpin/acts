use crate::{
    store::{
        db::local::{DbColumn, DbRow, DbSchema, DbType},
        Package,
    },
    Result,
};
use duckdb::{types::Value, Error as DbError, Result as DbResult};
impl DbSchema for Package {
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
            "size".to_string(),
            DbColumn {
                db_type: DbType::Int64,
                is_not_null: true,
                is_primary_key: false,
                ..Default::default()
            },
        ));
        map.push((
            "file_data".to_string(),
            DbColumn {
                db_type: DbType::Binary,
                is_not_null: true,
                is_primary_key: false,
                ..Default::default()
            },
        ));
        map.push((
            "create_time".to_string(),
            DbColumn {
                db_type: DbType::Int64,
                is_not_null: true,
                is_primary_key: false,
                ..Default::default()
            },
        ));
        map.push((
            "update_time".to_string(),
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

        Ok(map)
    }
}

impl DbRow for Package {
    fn id(&self) -> &str {
        &self.id
    }

    fn from_row<'a>(row: &duckdb::Row<'a>) -> DbResult<Package, DbError> {
        Ok(Package {
            id: row.get::<usize, String>(0).unwrap(),
            name: row.get::<usize, String>(1).unwrap(),
            size: row.get::<usize, u32>(2).unwrap(),
            file_data: row.get::<usize, Vec<u8>>(3).unwrap(),
            create_time: row.get::<usize, i64>(4).unwrap(),
            update_time: row.get::<usize, i64>(5).unwrap(),
            timestamp: row.get::<usize, i64>(6).unwrap(),
        })
    }

    fn to_values(&self) -> Result<Vec<(String, Value)>> {
        let mut ret = Vec::new();

        ret.push(("id".to_string(), Value::Text(self.id.clone())));
        ret.push(("name".to_string(), Value::Text(self.name.clone())));
        ret.push(("size".to_string(), Value::UInt(self.size)));
        ret.push(("file_data".to_string(), Value::Blob(self.file_data.clone())));
        ret.push(("create_time".to_string(), Value::BigInt(self.create_time)));
        ret.push(("update_time".to_string(), Value::BigInt(self.update_time)));
        ret.push(("timestamp".to_string(), Value::BigInt(self.timestamp)));

        Ok(ret)
    }
}
