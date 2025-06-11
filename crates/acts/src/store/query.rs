use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use std::str::FromStr;

use crate::ActError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Query {
    pub offset: usize,
    pub limit: usize,
    pub filter: Option<Filter>,
    #[serde(rename = "order_by")]
    pub order_by: Vec<OrderBy>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OrderBy {
    pub field: String,
    pub order: Sort,
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub enum Sort {
    #[serde(rename = "asc")]
    Asc,
    #[serde(rename = "desc")]
    Desc,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilterType {
    #[serde(rename = "and")]
    And,
    #[serde(rename = "or")]
    Or,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Filter {
    #[serde(rename = "type")]
    pub r#type: FilterType,
    pub exprs: Vec<FilterExpr>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FilterExpr {
    Filter(Filter),
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Expr {
    pub key: String,
    pub value: JsonValue,
    pub op: ExprOp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExprOp {
    /// equal
    EQ,

    /// not equal
    NE,

    /// less than
    LT,

    /// less and equal
    LE,

    /// greater then
    GT,

    /// greater and equal
    GE,

    /// match the input
    #[serde(rename = "match")]
    Match,
}

impl Expr {
    pub fn key(&self) -> &str {
        &self.key
    }
    pub fn value(&self) -> &JsonValue {
        &self.value
    }

    pub fn eq<T: Serialize>(key: &str, v: T) -> Self {
        Self {
            op: ExprOp::EQ,
            key: key.to_string(),
            value: json!(v),
        }
    }

    pub fn ne<T: Serialize>(key: &str, v: T) -> Self {
        Self {
            op: ExprOp::NE,
            key: key.to_string(),
            value: json!(v),
        }
    }

    pub fn gt<T: Serialize>(key: &str, v: T) -> Self {
        Self {
            op: ExprOp::GT,
            key: key.to_string(),
            value: json!(v),
        }
    }

    pub fn lt<T: Serialize>(key: &str, v: T) -> Self {
        Self {
            op: ExprOp::LT,
            key: key.to_string(),
            value: json!(v),
        }
    }

    pub fn le<T: Serialize>(key: &str, v: T) -> Self {
        Self {
            op: ExprOp::LE,
            key: key.to_string(),
            value: json!(v),
        }
    }

    pub fn ge<T: Serialize>(key: &str, v: T) -> Self {
        Self {
            op: ExprOp::GE,
            key: key.to_string(),
            value: json!(v),
        }
    }

    pub fn matches(key: &str, v: &str) -> Self {
        Self {
            op: ExprOp::Match,
            key: key.to_string(),
            value: json!(v),
        }
    }
}

impl Filter {
    pub fn expr(mut self, expr: Expr) -> Self {
        self.exprs.push(FilterExpr::Expr(expr));
        self
    }

    pub fn push(mut self, filter: Filter) -> Self {
        self.exprs.push(FilterExpr::Filter(filter));
        self
    }

    pub fn and() -> Filter {
        Filter {
            r#type: FilterType::And,
            exprs: Vec::new(),
        }
    }
    pub fn or() -> Filter {
        Filter {
            r#type: FilterType::Or,
            exprs: Vec::new(),
        }
    }
}

impl Default for Query {
    fn default() -> Self {
        Self::new()
    }
}

impl Query {
    pub fn new() -> Self {
        Query {
            offset: 0,
            limit: 100000, // default to a big number
            order_by: Vec::new(),
            filter: None,
        }
    }

    pub fn filter(mut self, filter: Filter) -> Self {
        self.filter = Some(filter);

        self
    }

    pub fn offset(mut self, offset: usize) -> Self {
        self.offset = offset;

        self
    }

    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = limit;

        self
    }

    pub fn order(mut self, field: &str, order: Sort) -> Self {
        self.order_by.push(OrderBy {
            field: field.to_string(),
            order,
        });

        self
    }

    pub fn get_order_by(&self) -> &Vec<OrderBy> {
        &self.order_by
    }
}

impl FromStr for OrderBy {
    type Err = serde_json::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(field) = s.strip_prefix('+') {
            Ok(OrderBy {
                field: field.to_string(),
                order: Sort::Asc,
            })
        } else if let Some(field) = s.strip_prefix('-') {
            Ok(OrderBy {
                field: field.to_string(),
                order: Sort::Desc,
            })
        } else {
            Ok(OrderBy {
                field: s.to_string(),
                order: Sort::Asc,
            })
        }
    }
}

impl TryFrom<JsonValue> for Query {
    type Error = ActError;
    fn try_from(value: JsonValue) -> Result<Self, Self::Error> {
        serde_json::from_value(value).map_err(|err| ActError::Convert(err.to_string()))
    }
}

impl TryFrom<Query> for JsonValue {
    type Error = ActError;
    fn try_from(value: Query) -> Result<Self, Self::Error> {
        serde_json::to_value(value).map_err(|err| ActError::Convert(err.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::Expr;
    use crate::{
        query::{OrderBy, Sort},
        store::{ExprOp, MessageStatus},
    };
    use serde_json::json;

    #[test]
    fn store_query_expr_eq_null() {
        let expr = Expr::eq("a", json!(null));
        assert_eq!(expr.key(), "a");
        assert_eq!(expr.value(), &json!(null));
        assert_eq!(expr.op, ExprOp::EQ);
    }

    #[test]
    fn store_query_expr_ne_null() {
        let expr = Expr::ne("a", json!(null));
        assert_eq!(expr.key(), "a");
        assert_eq!(expr.value(), &json!(null));
        assert_eq!(expr.op, ExprOp::NE);
    }

    #[test]
    fn store_query_expr_eq_str() {
        let expr = Expr::eq("a", "abc");
        assert_eq!(expr.key(), "a");
        assert_eq!(expr.value(), &json!("abc"));
        assert_eq!(expr.op, ExprOp::EQ);
    }

    #[test]
    fn store_query_expr_ne_str() {
        let expr = Expr::ne("a", "abc");
        assert_eq!(expr.key(), "a");
        assert_eq!(expr.value(), &json!("abc"));
        assert_eq!(expr.op, ExprOp::NE);
    }

    #[test]
    fn store_query_expr_eq_num() {
        let expr = Expr::eq("a", 5);
        assert_eq!(expr.key(), "a");
        assert_eq!(expr.value(), &json!(5));
        assert_eq!(expr.op, ExprOp::EQ);
    }

    #[test]
    fn store_query_expr_ne_num() {
        let expr = Expr::ne("a", 5);
        assert_eq!(expr.key(), "a");
        assert_eq!(expr.value(), &json!(5));
        assert_eq!(expr.op, ExprOp::NE);
    }

    #[test]
    fn store_query_expr_lt_num() {
        let expr = Expr::lt("a", 5);
        assert_eq!(expr.key(), "a");
        assert_eq!(expr.value(), &json!(5));
        assert_eq!(expr.op, ExprOp::LT);
    }

    #[test]
    fn store_query_expr_le_num() {
        let expr = Expr::le("a", 5);
        assert_eq!(expr.key(), "a");
        assert_eq!(expr.value(), &json!(5));
        assert_eq!(expr.op, ExprOp::LE);
    }

    #[test]
    fn store_query_expr_gt_num() {
        let expr = Expr::gt("a", 5);
        assert_eq!(expr.key(), "a");
        assert_eq!(expr.value(), &json!(5));
        assert_eq!(expr.op, ExprOp::GT);
    }

    #[test]
    fn store_query_expr_ge_num() {
        let expr = Expr::ge("a", 5);
        assert_eq!(expr.key(), "a");
        assert_eq!(expr.value(), &json!(5));
        assert_eq!(expr.op, ExprOp::GE);
    }

    #[test]
    fn store_query_expr_enum() {
        let expr = Expr::eq("a", MessageStatus::Acked);
        assert_eq!(expr.key(), "a");
        assert_eq!(expr.value(), &json!(MessageStatus::Acked));
    }

    #[test]
    fn store_query_expr_matches() {
        let expr = Expr::matches("a", "abc");
        assert_eq!(expr.key(), "a");
        assert_eq!(expr.op, ExprOp::Match);
        assert_eq!(expr.value(), &json!("abc"));
    }

    #[test]
    fn store_query_order_by_default() {
        let ob: OrderBy = "field1".parse().unwrap();
        assert_eq!(ob.field, "field1");
        assert_eq!(ob.order, Sort::Asc);
    }

    #[test]
    fn store_query_order_by_asc() {
        let ob: OrderBy = "+field1".parse().unwrap();
        assert_eq!(ob.field, "field1");
        assert_eq!(ob.order, Sort::Asc);
    }

    #[test]
    fn store_query_order_by_desc() {
        let ob: OrderBy = "-field1".parse().unwrap();
        assert_eq!(ob.field, "field1");
        assert_eq!(ob.order, Sort::Desc);
    }
}
