use serde::Serialize;
use serde_json::{Value, json};
use std::{collections::HashSet, slice::IterMut};

#[derive(Debug, Clone)]
pub struct Query {
    offset: usize,
    limit: usize,
    conds: Vec<Cond>,
    order_by: Vec<(String, bool)>,
}

#[derive(Debug, Clone)]
pub enum CondType {
    And,
    Or,
}

#[derive(Debug, Clone)]
pub struct Cond {
    pub r#type: CondType,
    pub conds: Vec<Expr>,
    pub result: HashSet<Box<[u8]>>,
}

#[derive(Debug, Clone, PartialEq)]
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
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub op: ExprOp,
    pub key: String,
    pub value: Value,
}

impl Expr {
    pub fn key(&self) -> &str {
        &self.key
    }
    pub fn value(&self) -> &Value {
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
}

impl Cond {
    pub fn or() -> Self {
        Self {
            r#type: CondType::Or,
            conds: Default::default(),
            result: HashSet::new(),
        }
    }

    pub fn and() -> Self {
        Self {
            r#type: CondType::And,
            conds: Default::default(),
            result: HashSet::new(),
        }
    }

    pub fn conds(&self) -> &Vec<Expr> {
        &self.conds
    }

    pub fn push(mut self, expr: Expr) -> Self {
        self.conds.push(expr);
        self
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
            conds: Vec::new(),
        }
    }

    pub fn queries_mut(&mut self) -> IterMut<'_, Cond> {
        self.conds.iter_mut()
    }

    pub fn queries(&mut self) -> &Vec<Cond> {
        &self.conds
    }

    pub fn calc(&self) -> HashSet<Box<[u8]>> {
        let mut result = HashSet::new();
        for cond in self.conds.iter() {
            if result.is_empty() {
                result = cond.result.clone();
            } else {
                result = result
                    .intersection(&cond.result)
                    .cloned()
                    .collect::<HashSet<_>>()
            }
        }
        result
    }

    pub fn push(mut self, cond: Cond) -> Self {
        self.conds.push(cond);

        self
    }

    pub fn set_offset(mut self, offset: usize) -> Self {
        self.offset = offset;

        self
    }

    pub fn set_limit(mut self, limit: usize) -> Self {
        self.limit = limit;

        self
    }

    pub fn set_order(mut self, order_by: &[(String, bool)]) -> Self {
        self.order_by = order_by.to_vec();

        self
    }

    pub fn push_order(mut self, order: &str, is_rev: bool) -> Self {
        self.order_by.push((order.to_string(), is_rev));

        self
    }

    pub fn limit(&self) -> usize {
        if self.limit == 0 {
            return 50;
        }

        self.limit
    }

    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn is_cond(&self) -> bool {
        !self.conds.is_empty()
    }

    pub fn order_by(&self) -> &Vec<(String, bool)> {
        &self.order_by
    }
}

#[cfg(test)]
mod tests {
    use super::Expr;
    use crate::store::{ExprOp, MessageStatus};
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
}
