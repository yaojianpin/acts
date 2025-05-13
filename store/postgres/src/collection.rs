mod event;
mod message;
mod model;
mod package;
mod proc;
mod task;

use std::sync::Arc;

pub use event::EventCollection;
pub use message::MessageCollection;
pub use model::ModelCollection;
pub use package::PackageCollection;
pub use proc::ProcCollection;
pub use task::TaskCollection;

use super::synclient::SynClient;
use acts::{ActError, query::*};
use sea_query::{
    Alias as SeaAlias, Cond as SeaCond, Condition, Expr as SeaExpr, IntoCondition, Value,
};

pub type DbConnection = Arc<SynClient>;

fn map_db_err(err: impl std::error::Error) -> ActError {
    ActError::Store(err.to_string())
}

fn into_query(q: &acts::query::Query) -> SeaCond {
    let mut filter = SeaCond::all();
    if q.is_cond() {
        let mut q = q.clone();
        let queries = q.queries();
        for (_index, cond) in queries.iter().enumerate() {
            let mut sea_cond = match cond.r#type {
                CondType::And => SeaCond::all(),
                CondType::Or => SeaCond::any(),
            };

            for (_index, expr) in cond.conds().iter().enumerate() {
                let cond = into_cond(expr);
                sea_cond = sea_cond.add(cond);
            }
            filter = filter.add(sea_cond);
        }
    }

    filter
}

fn into_cond(expr: &Expr) -> Condition {
    let key = &expr.key;
    let value = json_to_sea_value(expr.value.clone());
    let expr_condition = match expr.op {
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
    };

    expr_condition
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
