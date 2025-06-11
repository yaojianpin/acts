mod event;
mod message;
mod model;
mod package;
mod proc;
mod task;

pub use event::EventCollection;
pub use message::MessageCollection;
pub use model::ModelCollection;
pub use package::PackageCollection;
pub use proc::ProcCollection;
pub use task::TaskCollection;

use acts::{ActError, query::*};
use r2d2_sqlite::SqliteConnectionManager;
use sea_query::{
    Alias as SeaAlias, Cond as SeaCond, Condition, Expr as SeaExpr, IntoCondition, Value,
};

pub type DbConnection = r2d2::Pool<SqliteConnectionManager>;

fn map_db_err(err: impl std::error::Error) -> ActError {
    ActError::Store(err.to_string())
}

fn into_query(q: &acts::query::Query) -> SeaCond {
    let mut filter = SeaCond::all();
    if let Some(cond) = &q.filter {
        filter = filter.add(into_cond(cond));
    }

    filter
}

fn into_cond(cond: &Filter) -> Condition {
    let mut sea_cond = match cond.r#type {
        FilterType::And => SeaCond::all(),
        FilterType::Or => SeaCond::any(),
    };
    for cond_expr in cond.exprs.iter() {
        sea_cond = sea_cond.add(into_cond_expr(cond_expr));
    }

    sea_cond
}

fn into_cond_expr(cond: &FilterExpr) -> Condition {
    match cond {
        FilterExpr::Filter(cond) => into_cond(cond),
        FilterExpr::Expr(expr) => {
            let key = &expr.key;
            let value = json_to_sea_value(expr.value.clone());
            match expr.op {
                ExprOp::EQ => {
                    if expr.value.is_null() {
                        SeaExpr::col(SeaAlias::new(key)).is_null().into_condition()
                    } else {
                        SeaExpr::col(SeaAlias::new(key))
                            .eq(value.unwrap())
                            .into_condition()
                    }
                }
                ExprOp::NE => {
                    if expr.value.is_null() {
                        SeaExpr::col(SeaAlias::new(key))
                            .is_not_null()
                            .into_condition()
                    } else {
                        SeaExpr::col(SeaAlias::new(key))
                            .ne(value.unwrap())
                            .into_condition()
                    }
                }
                ExprOp::LT => SeaExpr::col(SeaAlias::new(key))
                    .lt(value.unwrap())
                    .into_condition(),
                ExprOp::LE => SeaExpr::col(SeaAlias::new(key))
                    .lte(value.unwrap())
                    .into_condition(),
                ExprOp::GT => SeaExpr::col(SeaAlias::new(key))
                    .gt(value.unwrap())
                    .into_condition(),
                ExprOp::GE => SeaExpr::col(SeaAlias::new(key))
                    .gte(value.unwrap())
                    .into_condition(),
                ExprOp::Match => {
                    let value = value.map(|v| match v {
                        Value::String(v) => v.map(|v| *v).unwrap_or("".to_string()),
                        v => v.to_string(),
                    });

                    let like = format!("%{}%", value.unwrap());
                    SeaExpr::col(SeaAlias::new(key)).like(like).into_condition()
                }
            }
        }
    }
}

fn json_to_sea_value(value: serde_json::Value) -> Option<Value> {
    match value {
        serde_json::Value::Bool(v) => Some(Value::Bool(Some(v))),
        serde_json::Value::Number(number) => {
            if let Some(v) = number.as_i64() {
                Some(Value::BigInt(Some(v)))
            } else if let Some(v) = number.as_f64() {
                Some(Value::Double(Some(v)))
            } else {
                Some(Value::Int(Some(0)))
            }
        }
        serde_json::Value::String(v) => Some(Value::String(Some(Box::new(v)))),
        _ => None,
    }
}
