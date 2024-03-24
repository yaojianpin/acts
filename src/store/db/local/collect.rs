use super::{database::Database, DbRow, DbSchema};
use crate::{
    store::{map_db_err, query::CondType, DbSet, Query},
    Result, ShareLock,
};
use duckdb::{params, params_from_iter};
use std::{fmt::Debug, marker::PhantomData};
use tracing::debug;

#[derive(Debug)]
pub struct Collect<T> {
    db: ShareLock<Database>,
    name: String,
    _t: PhantomData<T>,
}

impl<T> Collect<T>
where
    T: DbSchema,
{
    pub fn new(db: &ShareLock<Database>, name: &str) -> Self {
        db.write().unwrap().init(name, &T::schema().unwrap());
        Self {
            db: db.clone(),
            name: name.to_string(),
            _t: PhantomData::default(),
        }
    }
}

impl<'a, T> DbSet for Collect<T>
where
    T: DbSchema + DbRow + Debug + Send + Sync + Clone,
{
    type Item = T;
    fn exists(&self, id: &str) -> Result<bool> {
        debug!("local::{}.exists({})", self.name, id);
        let db = self.db.read().unwrap();
        let conn = db.pool().get().unwrap();

        let mut stmt = conn
            .prepare(&format!("select count(id) from {} where id = ?", self.name))
            .map_err(map_db_err)?;
        let result = stmt
            .query_row(params![id], |row| row.get::<usize, i64>(0))
            .map_err(map_db_err)?;
        return Ok(result > 0);
    }

    fn find(&self, id: &str) -> Result<T> {
        debug!("local::{}.find({})", self.name, id);
        let db = self.db.read().unwrap();
        let conn = db.pool().get().unwrap();

        let schema = T::schema()?;
        let keys: Vec<&str> = schema.iter().map(|(k, _)| k.as_str()).collect();
        let sql = format!("select {} from {} where id = ?", keys.join(","), self.name);
        let row = conn
            .prepare(&sql)
            .map_err(map_db_err)?
            .query_row(params![id], |row| Ok(T::from_row(row)))
            .map_err(map_db_err)?;
        let model = row.map_err(map_db_err)?;
        Ok(model)
    }

    fn query(&self, q: &Query) -> Result<Vec<T>> {
        debug!("local::{}.query({:?})", self.name, q);
        let db = self.db.read().unwrap();
        let conn = db.pool().get().unwrap();
        let mut filter = String::new();
        if q.is_cond() {
            let mut q = q.clone();

            let queries = q.queries();
            for (index, cond) in queries.iter().enumerate() {
                let typ = match cond.r#type {
                    CondType::And => "and",
                    CondType::Or => "or",
                };
                filter.push_str("(");
                for (index, expr) in cond.conds.iter().enumerate() {
                    filter.push_str(&format!("{} = '{}'", expr.key, expr.value));
                    if index != cond.conds.len() - 1 {
                        filter.push_str(&format!(" {typ} "));
                    }
                }
                filter.push_str(")");

                if index != queries.len() - 1 {
                    filter.push_str(&format!(" and "));
                }
            }
        }

        debug!("filter: {filter}");
        let schema = T::schema()?;
        let keys: Vec<&str> = schema.iter().map(|(k, _)| k.as_str()).collect();
        let mut sql = format!(
            "select {} from {} limit {} offset {}",
            keys.join(","),
            self.name,
            q.limit(),
            q.offset(),
        );
        if !filter.is_empty() {
            sql = format!(
                "select {} from {} where {} limit {} offset {}",
                keys.join(","),
                self.name,
                filter,
                q.limit(),
                q.offset(),
            );
        }

        let ret = conn
            .prepare(&sql)
            .map_err(map_db_err)?
            .query_map([], |row| T::from_row(row))
            .map_err(map_db_err)?
            .map(|v| v.unwrap())
            .collect::<Vec<_>>();

        Ok(ret)
    }

    fn create(&self, model: &T) -> Result<bool> {
        debug!("local::{}.create({})", self.name, model.id());
        let db = self.db.write().unwrap();
        let conn = db.pool().get().unwrap();

        let mut keys = Vec::new();
        let mut values = Vec::new();
        for (k, v) in model.to_values()? {
            keys.push(k);
            values.push(v);
        }

        let ret = conn
            .execute(
                &format!(
                    "insert into {} ( {} ) values ( {} )",
                    self.name,
                    keys.join(","),
                    repeat_var(values.len())
                ),
                params_from_iter(values),
            )
            .map_err(map_db_err)?;
        Ok(ret > 0)
    }
    fn update(&self, model: &T) -> Result<bool> {
        debug!("local::{}.update({})", self.name, model.id());
        let db = self.db.write().unwrap();
        let conn = db.pool().get().unwrap();

        let mut keys = Vec::new();
        let mut values = Vec::new();
        for (k, v) in model.to_values()? {
            if k == "id" {
                continue;
            }
            keys.push(format!("{} = ?", k.as_str()));
            values.push(v);
        }

        let ret = conn
            .execute(
                &format!(
                    "update {} set {} where id = '{}'",
                    self.name,
                    keys.join(","),
                    model.id()
                ),
                params_from_iter(values),
            )
            .map_err(map_db_err)?;

        Ok(ret > 0)
    }
    fn delete(&self, id: &str) -> Result<bool> {
        debug!("local::{}.delete({})", self.name, id);
        let db = self.db.write().unwrap();
        let conn = db.pool().get().unwrap();

        let ret = conn
            .execute(
                &format!("delete from {} where id = ?", self.name),
                params![id],
            )
            .map_err(map_db_err)?;

        Ok(ret > 0)
    }
}

fn repeat_var(len: usize) -> String {
    let mut var = "?,".repeat(len);
    var.pop();

    var
}
