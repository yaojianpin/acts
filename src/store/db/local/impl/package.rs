use std::str::FromStr;

use crate::{
    ActRunAs, Result,
    store::{
        Package,
        db::local::{DbColumn, DbRow, DbSchema, DbType},
    },
};
use rusqlite::{Error as DbError, Result as DbResult, Row, types::Value};
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
                "desc".to_string(),
                DbColumn {
                    db_type: DbType::Text,
                    ..Default::default()
                },
            ),
            (
                "icon".to_string(),
                DbColumn {
                    db_type: DbType::Text,
                    ..Default::default()
                },
            ),
            (
                "doc".to_string(),
                DbColumn {
                    db_type: DbType::Text,
                    ..Default::default()
                },
            ),
            (
                "version".to_string(),
                DbColumn {
                    db_type: DbType::Text,
                    ..Default::default()
                },
            ),
            (
                "schema".to_string(),
                DbColumn {
                    db_type: DbType::Text,
                    ..Default::default()
                },
            ),
            (
                "run_as".to_string(),
                DbColumn {
                    db_type: DbType::Text,
                    ..Default::default()
                },
            ),
            (
                "groups".to_string(),
                DbColumn {
                    db_type: DbType::Text,
                    ..Default::default()
                },
            ),
            (
                "catalog".to_string(),
                DbColumn {
                    db_type: DbType::Text,
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
            (
                "built_in".to_string(),
                DbColumn {
                    db_type: DbType::Int8,
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
            desc: row.get::<usize, String>(1).unwrap(),

            icon: row.get::<usize, String>(2).unwrap(),
            doc: row.get::<usize, String>(3).unwrap(),
            version: row.get::<usize, String>(4).unwrap(),
            schema: row.get::<usize, String>(5).unwrap(),
            run_as: ActRunAs::from_str(&row.get::<usize, String>(6).unwrap()).unwrap(),
            groups: row.get::<usize, String>(7).unwrap(),
            catalog: crate::ActPackageCatalog::from_str(&row.get::<usize, String>(8).unwrap())
                .unwrap(),
            create_time: row.get::<usize, i64>(9).unwrap(),
            update_time: row.get::<usize, i64>(10).unwrap(),
            timestamp: row.get::<usize, i64>(11).unwrap(),
            built_in: row.get::<usize, i32>(12).unwrap() > 0,
        })
    }

    fn to_values(&self) -> Result<Vec<(String, Value)>> {
        let ret = vec![
            ("id".to_string(), Value::Text(self.id.clone())),
            ("desc".to_string(), Value::Text(self.desc.clone())),
            ("icon".to_string(), Value::Text(self.icon.clone())),
            ("doc".to_string(), Value::Text(self.doc.clone())),
            ("version".to_string(), Value::Text(self.version.clone())),
            ("schema".to_string(), Value::Text(self.schema.clone())),
            (
                "run_as".to_string(),
                Value::Text(self.run_as.as_ref().to_string()),
            ),
            ("groups".to_string(), Value::Text(self.groups.clone())),
            (
                "catalog".to_string(),
                Value::Text(self.catalog.as_ref().to_string()),
            ),
            ("create_time".to_string(), Value::Integer(self.create_time)),
            ("update_time".to_string(), Value::Integer(self.update_time)),
            ("timestamp".to_string(), Value::Integer(self.timestamp)),
            (
                "built_in".to_string(),
                Value::Integer(if self.built_in { 1 } else { 0 }),
            ),
        ];

        Ok(ret)
    }
}
