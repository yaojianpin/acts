use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use std::str::FromStr;

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
        if let Some(stripped) = s.strip_suffix('!') {
            Ok(OrderBy {
                field: stripped.to_string(),
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
    type Error = anyhow::Error;
    fn try_from(value: JsonValue) -> Result<Self, Self::Error> {
        serde_json::from_value(value).map_err(|err| anyhow::anyhow!(err.to_string()))
    }
}

impl TryFrom<Query> for JsonValue {
    type Error = anyhow::Error;
    fn try_from(value: Query) -> Result<Self, Self::Error> {
        serde_json::to_value(value).map_err(|err| anyhow::anyhow!(err.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::Expr;
    use super::ExprOp;
    use acts::data::DeliveryStatus;
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
        let expr = Expr::eq("a", DeliveryStatus::Acked);
        assert_eq!(expr.key(), "a");
        assert_eq!(expr.value(), &json!(DeliveryStatus::Acked));
    }

    #[test]
    fn store_query_expr_matches() {
        let expr = Expr::matches("a", "abc");
        assert_eq!(expr.key(), "a");
        assert_eq!(expr.op, ExprOp::Match);
        assert_eq!(expr.value(), &json!("abc"));
    }
}

#[cfg(test)]
mod query_tests {
    use super::{Expr, ExprOp, Filter, FilterExpr, FilterType, OrderBy, Query, Sort};
    use serde_json::{Value as JsonValue, json};
    use std::str::FromStr;

    #[test]
    fn defaults_and_builders() {
        let query = Query::default();
        assert_eq!(query.offset, 0);
        assert_eq!(query.limit, 100000);
        assert!(query.filter.is_none());
        assert!(query.order_by.is_empty());

        let filter = Filter::and()
            .expr(Expr::eq("a", 1))
            .push(Filter::or().expr(Expr::matches("b", "x")));
        let query = Query::new()
            .offset(5)
            .limit(10)
            .filter(filter)
            .order("a", Sort::Asc)
            .order("b", Sort::Desc);

        assert_eq!(query.offset, 5);
        assert_eq!(query.limit, 10);

        let filter = query.filter.as_ref().expect("the filter is kept");
        assert_eq!(filter.r#type, FilterType::And);
        assert_eq!(filter.exprs.len(), 2);
        match &filter.exprs[0] {
            FilterExpr::Expr(expr) => {
                assert_eq!(expr.key, "a");
                assert_eq!(expr.op, ExprOp::EQ);
                assert_eq!(expr.value, json!(1));
            }
            other => panic!("expected a leaf expression, got {other:?}"),
        }
        match &filter.exprs[1] {
            FilterExpr::Filter(nested) => {
                assert_eq!(nested.r#type, FilterType::Or);
                assert_eq!(nested.exprs.len(), 1);
            }
            other => panic!("expected a nested filter, got {other:?}"),
        }

        let order = query.get_order_by();
        assert_eq!(order.len(), 2);
        assert_eq!(order[0].field, "a");
        assert_eq!(order[0].order, Sort::Asc);
        assert_eq!(order[1].field, "b");
        assert_eq!(order[1].order, Sort::Desc);
    }

    #[test]
    fn order_by_parses_ascending_and_bang_descending() {
        let asc = OrderBy::from_str("name").unwrap();
        assert_eq!(asc.field, "name");
        assert_eq!(asc.order, Sort::Asc);

        let desc = OrderBy::from_str("name!").unwrap();
        assert_eq!(desc.field, "name");
        assert_eq!(desc.order, Sort::Desc);

        assert_eq!(OrderBy::from_str("name").unwrap().order, Sort::Asc);
    }

    #[test]
    fn serde_uses_the_wire_names() {
        let query = Query::new()
            .filter(Filter::or().expr(Expr::ne("x", "y")).expr(Expr::ge("n", 1)))
            .order("a", Sort::Asc)
            .order("b", Sort::Desc);

        let value = serde_json::to_value(&query).unwrap();
        assert_eq!(value["offset"], json!(0));
        assert_eq!(value["limit"], json!(100000));
        assert_eq!(
            value["order_by"],
            json!([
                {"field": "a", "order": "asc"},
                {"field": "b", "order": "desc"}
            ])
        );
        assert_eq!(value["filter"]["type"], json!("or"));
        assert_eq!(
            value["filter"]["exprs"][0],
            json!({"key": "x", "value": "y", "op": "ne"})
        );
        assert_eq!(
            value["filter"]["exprs"][1],
            json!({"key": "n", "value": 1, "op": "ge"})
        );

        let back: Query = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(back.order_by.len(), 2);
        assert_eq!(back.filter.as_ref().unwrap().exprs.len(), 2);

        // every operator has its lower-case wire name, including `match`
        for (op, text) in [
            (ExprOp::EQ, "eq"),
            (ExprOp::NE, "ne"),
            (ExprOp::LT, "lt"),
            (ExprOp::LE, "le"),
            (ExprOp::GT, "gt"),
            (ExprOp::GE, "ge"),
            (ExprOp::Match, "match"),
        ] {
            assert_eq!(serde_json::to_value(&op).unwrap(), json!(text));
            assert_eq!(serde_json::from_value::<ExprOp>(json!(text)).unwrap(), op);
        }

        // filter types and untagged sub-filters roundtrip
        assert_eq!(serde_json::to_value(FilterType::And).unwrap(), json!("and"));
        assert_eq!(serde_json::to_value(FilterType::Or).unwrap(), json!("or"));
        let nested = FilterExpr::Filter(Filter::and());
        let encoded = serde_json::to_value(&nested).unwrap();
        assert_eq!(
            serde_json::from_value::<FilterExpr>(encoded).unwrap(),
            nested
        );
    }

    #[test]
    fn query_converts_to_and_from_json() {
        let query = Query::new().offset(2).limit(3).order("id", Sort::Desc);
        let value = JsonValue::try_from(query).unwrap();
        let back = Query::try_from(value.clone()).unwrap();
        assert_eq!(back.offset, 2);
        assert_eq!(back.limit, 3);
        assert_eq!(back.order_by[0].field, "id");

        let err = Query::try_from(json!({"offset": "not a number"})).unwrap_err();
        assert!(!err.to_string().is_empty());
    }
}
