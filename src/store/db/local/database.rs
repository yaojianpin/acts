use crate::store::db::local::{DbColumn, DbType};
#[allow(unused_imports)]
use duckdb::{params, AccessMode, Config, DuckdbConnectionManager};
#[allow(unused_imports)]
use std::{fs, path::Path};
use tracing::debug;

pub struct Database {
    path: String,
    pool: r2d2::Pool<DuckdbConnectionManager>,
}

impl std::fmt::Debug for Database {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Database").field("db", &self.path).finish()
    }
}

impl Database {
    #[allow(unused_variables)]
    pub fn new(path: &str, name: &str) -> Self {
        // the db path will be conflict in tokio::test
        // just use memory mode to test
        #[cfg(not(test))]
        {
            fs::create_dir_all(path).unwrap();
            let config = Config::default()
                .access_mode(AccessMode::ReadWrite)
                .unwrap();
            let manager =
                DuckdbConnectionManager::file_with_flags(Path::new(path).join(name), config)
                    .unwrap();
            let pool = r2d2::Pool::new(manager).unwrap();

            Self {
                pool,
                path: path.to_string(),
            }
        }

        #[cfg(test)]
        {
            let manager = DuckdbConnectionManager::memory().unwrap();
            let pool = r2d2::Pool::new(manager).unwrap();
            Self {
                pool,
                path: path.to_string(),
            }
        }
    }

    pub fn pool(&self) -> &r2d2::Pool<DuckdbConnectionManager> {
        &self.pool
    }

    pub fn init(&mut self, name: &str, schema: &Vec<(String, DbColumn)>) {
        let mut conn = self.pool().get().unwrap();
        let mut sql = String::new();
        sql.push_str(&format!("create table IF NOT EXISTS {} ", name));
        sql.push_str("(");

        let len = schema.len();
        let mut idx_sqls = Vec::new();
        for (index, (key, col)) in schema.iter().enumerate() {
            sql.push_str(&format!(
                "{key} {}",
                match col.db_type {
                    DbType::Boolean => "BOOLEAN".to_string(),
                    DbType::Double => "DOUBLE".to_string(),
                    DbType::Decimal(width, scale) => format!("DECIMAL({width},{scale})"),
                    DbType::Int32 => "INTEGER".to_string(),
                    DbType::Int64 => "BIGINT".to_string(),
                    DbType::Text => "VARCHAR".to_string(),
                    DbType::Binary => "BLOB".to_string(),
                }
            ));

            if col.is_not_null {
                sql.push_str(" NOT NULL ");
            }

            if col.is_primary_key {
                sql.push_str(" PRIMARY KEY ");
            }

            if col.is_unique {
                sql.push_str(" UNIQUE ");
            }

            if let Some(default) = &col.default {
                sql.push_str(" DEFAULT ");
                sql.push_str(default);
            }

            if index < len - 1 {
                sql.push_str(",");
            }

            if col.is_index {
                idx_sqls.push(format!(
                    "create {} index idx_{}_{} on {} ({})",
                    if col.is_unique { "UNIQUE" } else { "" },
                    name,
                    key,
                    name,
                    key
                ));
            }
        }
        sql.push_str(");");
        debug!("sql={}", sql);
        let tr = conn.transaction().unwrap();
        if let Ok(affect_count) = tr.execute(&sql, params![]) {
            if affect_count > 0 {
                for idx_sql in idx_sqls {
                    tr.execute(&idx_sql, params![]).unwrap();
                }
            }
        }

        tr.commit().unwrap();
    }

    pub fn close(&self) {}
}
