use super::{database::Database, DbRow, DbSchema};
use crate::{
    store::{map_db_err, query::CondType, DbSet, Expr, ExprOp, PageData, Query},
    ActError, Result, ShareLock,
};
use rusqlite::{params, params_from_iter};
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
            _t: PhantomData,
        }
    }
}

impl<T> DbSet for Collect<T>
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
        Ok(result > 0)
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

    fn query(&self, q: &Query) -> Result<PageData<T>> {
        debug!("local::{}.query({:?})", self.name, q);
        let db = self.db.read().unwrap();
        let conn = db.pool().get().unwrap();
        let schema = T::schema()?;
        let keys: Vec<&str> = schema.iter().map(|(k, _)| k.as_str()).collect();
        let mut filter = String::new();
        if q.is_cond() {
            let mut q = q.clone();

            let queries = q.queries();
            for (index, cond) in queries.iter().enumerate() {
                let typ = match cond.r#type {
                    CondType::And => "and",
                    CondType::Or => "or",
                };
                filter.push('(');
                for (index, expr) in cond.conds().iter().enumerate() {
                    if !keys.contains(&expr.key()) {
                        return Err(ActError::Store(format!(
                            "cannot find key `{}` in {}, the available keys should be `{}`",
                            expr.key(),
                            self.name,
                            keys.join(",")
                        )));
                    }
                    filter.push_str(&expr.sql()?);
                    if index != cond.conds().len() - 1 {
                        filter.push_str(&format!(" {typ} "));
                    }
                }
                filter.push(')');

                if index != queries.len() - 1 {
                    filter.push_str(" and ");
                }
            }
        }

        let mut count_sql = format!("select count(id) from {} ", self.name);
        let mut sql = format!("select {} from {} ", keys.join(","), self.name);

        if !filter.is_empty() {
            count_sql.push_str(&format!(" where {} ", filter));
            sql.push_str(&format!(" where {} ", filter));
        }

        if !q.order_by().is_empty() {
            let len = q.order_by().len();
            sql.push_str(" order by ");
            for (index, (order, rev)) in q.order_by().iter().enumerate() {
                if !keys.contains(&order.as_str()) {
                    return Err(ActError::Store(format!(
                        "cannot find key `{order}` in {}, the available keys should be `{}`",
                        self.name,
                        keys.join(",")
                    )));
                }
                if index == len - 1 {
                    sql.push_str(&format!(
                        " {} {} ",
                        order,
                        if *rev { "desc" } else { "asc" }
                    ));
                } else {
                    sql.push_str(&format!(
                        " {} {},",
                        order,
                        if *rev { "desc" } else { "asc" }
                    ));
                }
            }
        }
        sql.push_str(&format!(" limit {} offset {} ", q.limit(), q.offset()));

        debug!("sql: {sql}");
        let count = conn
            .prepare(&count_sql)
            .map_err(map_db_err)?
            .query_row::<usize, _, _>([], |row| row.get(0))
            .map_err(map_db_err)?;
        let page_count = count.div_ceil(q.limit());
        let page_num = q.offset() / q.limit() + 1;
        let data = PageData {
            count,
            page_size: q.limit(),
            page_num,
            page_count,
            rows: conn
                .prepare(&sql)
                .map_err(map_db_err)?
                .query_map([], |row| T::from_row(row))
                .map_err(map_db_err)?
                .map(|v| v.unwrap())
                .collect::<Vec<_>>(),
        };
        Ok(data)
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

impl Expr {
    pub fn sql(&self) -> Result<String> {
        let key = &self.key;
        let op = match self.op {
            ExprOp::EQ => "=",
            ExprOp::NE => "!=",
            ExprOp::LT => "<",
            ExprOp::LE => "<=",
            ExprOp::GT => ">",
            ExprOp::GE => ">=",
        };
        match &self.value {
            serde_json::Value::Null => {
                if self.op == ExprOp::EQ {
                    return Ok(format!("{key} is null"));
                } else if self.op == ExprOp::NE {
                    return Ok(format!("{key} is not null"));
                }
                Err(crate::ActError::Store(format!(
                    "the operation({op}) is not support for null"
                )))
            }
            serde_json::Value::Bool(v) => {
                if self.op == ExprOp::EQ || self.op == ExprOp::NE {
                    return Ok(format!("{key} {op} {v}"));
                }
                Err(crate::ActError::Store(format!(
                    "the operation({op}) is not support for bool"
                )))
            }
            serde_json::Value::Number(v) => Ok(format!("{key} {op} {v}")),
            serde_json::Value::String(v) => {
                if self.op == ExprOp::EQ || self.op == ExprOp::NE {
                    return Ok(format!("{key} {op} '{v}'"));
                }
                Err(crate::ActError::Store(format!(
                    "the operation({op}) is not support for string"
                )))
            }
            v => Err(crate::ActError::Store(format!(
                "not support sql value for '{v}'"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::store::{Expr, MessageStatus};
    use serde_json::json;

    #[test]
    fn store_query_expr_eq_sql_null() {
        let expr = Expr::eq("a", json!(null));
        assert_eq!(expr.key(), "a");
        assert_eq!(expr.sql().unwrap(), "a is null");
    }

    #[test]
    fn store_query_expr_eq_sql_not_null() {
        let expr = Expr::ne("a", json!(null));
        assert_eq!(expr.key(), "a");
        assert_eq!(expr.sql().unwrap(), "a is not null");
    }

    #[test]
    fn store_query_expr_eq_sql_str() {
        let expr = Expr::eq("a", "abc");
        assert_eq!(expr.key(), "a");
        assert_eq!(expr.sql().unwrap(), "a = 'abc'");
    }

    #[test]
    fn store_query_expr_ne_sql_str() {
        let expr = Expr::ne("a", "abc");
        assert_eq!(expr.key(), "a");
        assert_eq!(expr.sql().unwrap(), "a != 'abc'");
    }

    #[test]
    fn store_query_expr_eq_sql_num() {
        let expr = Expr::eq("a", 5);
        assert_eq!(expr.key(), "a");
        assert_eq!(expr.sql().unwrap(), "a = 5");
    }

    #[test]
    fn store_query_expr_ne_sql_num() {
        let expr = Expr::ne("a", 5);
        assert_eq!(expr.key(), "a");
        assert_eq!(expr.sql().unwrap(), "a != 5");
    }

    #[test]
    fn store_query_expr_lt_sql_num() {
        let expr = Expr::lt("a", 5);
        assert_eq!(expr.key(), "a");
        assert_eq!(expr.sql().unwrap(), "a < 5");
    }

    #[test]
    fn store_query_expr_le_sql_num() {
        let expr = Expr::le("a", 5);
        assert_eq!(expr.key(), "a");
        assert_eq!(expr.sql().unwrap(), "a <= 5");
    }

    #[test]
    fn store_query_expr_gt_sql_num() {
        let expr = Expr::gt("a", 5);
        assert_eq!(expr.key(), "a");
        assert_eq!(expr.sql().unwrap(), "a > 5");
    }

    #[test]
    fn store_query_expr_ge_sql_num() {
        let expr = Expr::ge("a", 5);
        assert_eq!(expr.key(), "a");
        assert_eq!(expr.sql().unwrap(), "a >= 5");
    }

    #[test]
    fn store_query_expr_eq_sql_enum() {
        let expr = Expr::eq("a", MessageStatus::Created);
        assert_eq!(expr.sql().unwrap(), "a = 0");

        let expr = Expr::eq("a", MessageStatus::Acked);
        assert_eq!(expr.sql().unwrap(), "a = 1");

        let expr = Expr::eq("a", MessageStatus::Completed);
        assert_eq!(expr.sql().unwrap(), "a = 2");
    }
}
