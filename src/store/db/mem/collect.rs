use serde::de::DeserializeOwned;
use tracing::debug;

use crate::store::query::CondType;
use crate::store::{map_db_err, Cond, Expr, ExprOp};
use crate::{ActError, DbSet, Query, Result, ShareLock};
use serde_json::Value as JsonValue;
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
            _t: PhantomData::default(),
        }
    }
}

impl<T> DbSet for Collect<T>
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
            .map(|iter| {
                let model = map_to_model::<Self::Item>(iter).unwrap();
                model
            })
            .ok_or(ActError::Store(format!(
                "cannot find {} by '{}'",
                self.name, id
            )))
    }

    fn query(&self, q: &Query) -> crate::Result<Vec<Self::Item>> {
        debug!("mem::{}.query({:?})", self.name, q);
        let db = self.db.read().unwrap();
        if q.is_cond() {
            let mut q = q.clone();
            for cond in q.queries_mut() {
                let mut result = HashSet::new();
                for expr in cond.conds().iter() {
                    for (k, v) in db.iter() {
                        let prop_value = v.get(expr.key()).unwrap();
                        let cond_value = expr.value();

                        if expr.op(prop_value, cond_value) {
                            result.insert(k.as_bytes().to_vec().into_boxed_slice());
                        }
                    }
                }
                cond.calc(&result);
            }

            let keys = q
                .calc()
                .into_iter()
                .skip(q.offset())
                .take(q.limit())
                .collect::<Vec<_>>();
            let ret = db
                .iter()
                .filter_map(|(k, v)| {
                    if keys.contains(&k.as_bytes().to_vec().into_boxed_slice()) {
                        return Some(map_to_model(v).unwrap());
                    }
                    None
                })
                .collect::<Vec<_>>();

            return Ok(ret);
        }
        Ok(db
            .values()
            .map(|v| map_to_model::<Self::Item>(v).unwrap())
            .skip(q.offset())
            .take(q.limit())
            .collect::<Vec<_>>())
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

impl Cond {
    pub fn calc(&mut self, v: &HashSet<Box<[u8]>>) {
        match self.r#type {
            CondType::And => {
                if self.result.len() == 0 {
                    self.result = v.clone();
                } else {
                    self.result = self.result.intersection(v).cloned().collect::<HashSet<_>>()
                }
            }
            CondType::Or => {
                if self.result.len() == 0 {
                    self.result = v.clone();
                } else {
                    self.result = self.result.union(v).cloned().collect::<HashSet<_>>()
                }
            }
        }
    }
}

impl Expr {
    pub fn op(&self, l: &serde_json::Value, r: &serde_json::Value) -> bool {
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
                return false;
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
                return false;
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
                return false;
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
                return false;
            }
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
