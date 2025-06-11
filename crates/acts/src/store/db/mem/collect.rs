use serde::de::DeserializeOwned;
use tracing::debug;

use crate::store::query::FilterType;
use crate::store::{Expr, ExprOp, Filter, PageData, map_db_err};
use crate::{ActError, DbCollection, Result, ShareLock, store::query::*};
use serde_json::Value as JsonValue;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Debug;
use std::marker::PhantomData;
use std::sync::{Arc, RwLock};

use super::DbDocument;

#[derive(Debug)]
pub struct Collect<T> {
    name: String,
    db: ShareLock<BTreeMap<String, HashMap<String, JsonValue>>>,
    _t: PhantomData<T>,
}

impl<T> Collect<T> {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            db: Arc::new(RwLock::new(BTreeMap::new())),
            _t: PhantomData,
        }
    }
}

impl<T> DbCollection for Collect<T>
where
    T: DbDocument + Send + Sync + Clone + Debug,
{
    type Item = T;

    fn exists(&self, id: &str) -> crate::Result<bool> {
        debug!("mem::{}.exists({:?})", self.name, id);
        Ok(self.db.read().unwrap().contains_key(id))
    }

    fn find(&self, id: &str) -> Result<Self::Item> {
        debug!("mem::{}.find({:?})", self.name, id);
        self.db
            .read()
            .unwrap()
            .get(id)
            .map(|iter| map_to_model::<Self::Item>(iter).unwrap())
            .ok_or(ActError::Store(format!(
                "cannot find {} by '{}'",
                self.name, id
            )))
    }

    fn query(&self, q: &Query) -> crate::Result<PageData<Self::Item>> {
        debug!("mem::{}.query({:?})", self.name, q);
        let db = self.db.read().unwrap();
        #[allow(unused_assignments)]
        let mut rows = vec![];

        if let Some(cond) = &q.filter {
            let data = db.iter().collect::<Vec<_>>();
            let items = cond.calc(&self.name, &data)?;
            #[allow(unused_assignments)]
            {
                rows = db
                    .iter()
                    .filter_map(|(k, v)| {
                        if items.contains(&k.as_bytes().to_vec().into_boxed_slice()) {
                            return Some(v);
                        }
                        None
                    })
                    .collect::<Vec<_>>();
            }
        } else {
            rows = db.iter().map(|(_, v)| v).collect::<Vec<_>>();
        }
        // order the rows
        if !q.get_order_by().is_empty() {
            rows.sort_by(|a, b| {
                let mut ret = Ordering::Equal;
                for ob in q.get_order_by() {
                    match ob.order {
                        Sort::Asc => {
                            ret = ret.then(
                                a.get(&ob.field)
                                    .unwrap()
                                    .to_string()
                                    .cmp(&b.get(&ob.field).unwrap().to_string()),
                            );
                        }
                        Sort::Desc => {
                            ret = ret.then(
                                b.get(&ob.field)
                                    .unwrap()
                                    .to_string()
                                    .cmp(&a.get(&ob.field).unwrap().to_string()),
                            );
                        }
                    }
                }

                ret
            });
        }

        let count = rows.len();
        let page_count = count.div_ceil(q.limit);
        let page_num = q.offset / q.limit + 1;
        let data = PageData {
            count,
            page_size: q.limit,
            page_num,
            page_count,
            rows: rows
                .iter()
                .skip(q.offset)
                .take(q.limit)
                .map(|row| map_to_model::<Self::Item>(row).unwrap())
                .collect::<Vec<_>>(),
        };
        Ok(data)
    }

    fn create(&self, data: &Self::Item) -> Result<bool> {
        debug!("mem::{}.create({:?})", self.name, data);
        self.db
            .write()
            .unwrap()
            .insert(data.id().to_string(), data.doc()?);
        Ok(true)
    }

    fn update(&self, data: &Self::Item) -> Result<bool> {
        debug!("mem::{}.update({:?})", self.name, data);
        self.db
            .write()
            .unwrap()
            .entry(data.id().to_string())
            .and_modify(|iter| *iter = data.doc().unwrap());
        Ok(true)
    }

    fn delete(&self, id: &str) -> crate::Result<bool> {
        debug!("mem::{}.delete({:?})", self.name, id);
        self.db.write().unwrap().remove(id);
        Ok(true)
    }
}

impl Filter {
    pub fn calc(
        &self,
        name: &str,
        iters: &[(&String, &HashMap<String, serde_json::Value>)],
    ) -> Result<HashSet<Box<[u8]>>> {
        let mut result = HashSet::new();

        for cond in &self.exprs {
            let v = cond.calc(name, iters)?;
            if result.is_empty() {
                result = v;
            } else {
                match self.r#type {
                    FilterType::And => {
                        result = result.intersection(&v).cloned().collect::<HashSet<_>>()
                    }
                    FilterType::Or => result = result.union(&v).cloned().collect::<HashSet<_>>(),
                }
            }
        }
        Ok(result)
    }
}

impl Expr {
    pub fn op(&self, l: &serde_json::Value, r: &serde_json::Value) -> bool {
        debug!("Expr.op op={:?}, l={l}, r={r}", self.op);
        match &self.op {
            ExprOp::EQ => l == r,
            ExprOp::NE => l != r,
            ExprOp::LT => {
                if let (serde_json::Value::Number(v1), serde_json::Value::Number(v2)) = (l, r) {
                    if v1.is_f64() {
                        return v1.as_f64().unwrap() < v2.as_f64().unwrap_or_default();
                    } else if v1.is_i64() {
                        return v1.as_i64().unwrap() < v2.as_i64().unwrap_or_default();
                    } else if v1.is_u64() {
                        return v1.as_u64().unwrap() < v2.as_u64().unwrap_or_default();
                    }
                }
                false
            }
            ExprOp::LE => {
                if let (serde_json::Value::Number(v1), serde_json::Value::Number(v2)) = (l, r) {
                    if v1.is_f64() {
                        return v1.as_f64().unwrap() <= v2.as_f64().unwrap_or_default();
                    } else if v1.is_i64() {
                        return v1.as_i64().unwrap() <= v2.as_i64().unwrap_or_default();
                    } else if v1.is_u64() {
                        return v1.as_u64().unwrap() <= v2.as_u64().unwrap_or_default();
                    }
                }
                false
            }
            ExprOp::GT => {
                if let (serde_json::Value::Number(v1), serde_json::Value::Number(v2)) = (l, r) {
                    if v1.is_f64() {
                        return v1.as_f64().unwrap() > v2.as_f64().unwrap_or_default();
                    } else if v1.is_i64() {
                        return v1.as_i64().unwrap() > v2.as_i64().unwrap_or_default();
                    } else if v1.is_u64() {
                        return v1.as_u64().unwrap() > v2.as_u64().unwrap_or_default();
                    }
                }
                false
            }
            ExprOp::GE => {
                if let (serde_json::Value::Number(v1), serde_json::Value::Number(v2)) = (l, r) {
                    if v1.is_f64() {
                        return v1.as_f64().unwrap() >= v2.as_f64().unwrap_or_default();
                    } else if v1.is_i64() {
                        return v1.as_i64().unwrap() >= v2.as_i64().unwrap_or_default();
                    } else if v1.is_u64() {
                        return v1.as_u64().unwrap() >= v2.as_u64().unwrap_or_default();
                    }
                }
                false
            }
            ExprOp::Match => {
                let value = match r {
                    JsonValue::String(v) => v,
                    v => &v.to_string(),
                };
                l.to_string().contains(value)
            }
        }
    }
}

impl FilterExpr {
    pub fn calc(
        &self,
        name: &str,
        iters: &[(&String, &HashMap<String, serde_json::Value>)],
    ) -> Result<HashSet<Box<[u8]>>> {
        let get_expr_ret = |expr: &Expr| -> Result<HashSet<Box<[u8]>>> {
            let mut result = HashSet::new();
            for (k, v) in iters {
                let prop_value = v.get(expr.key()).ok_or(ActError::Store(format!(
                    "cannot find key `{}` in {}",
                    expr.key(),
                    name
                )))?;
                let cond_value = expr.value();

                if expr.op(prop_value, cond_value) {
                    result.insert(k.as_bytes().to_vec().into_boxed_slice());
                }
            }
            Ok(result)
        };
        match self {
            FilterExpr::Filter(cond) => cond.calc(name, iters),
            FilterExpr::Expr(expr) => get_expr_ret(expr),
        }
    }
}

fn map_to_model<T>(map: &HashMap<String, JsonValue>) -> Result<T>
where
    T: DeserializeOwned,
{
    let mut value = serde_json::Map::new();
    for (k, v) in map {
        value.insert(k.to_string(), v.clone());
    }
    serde_json::from_value(JsonValue::Object(value)).map_err(map_db_err)
}
