use super::map_db_err;
use crate::store::{
    DbCollection, DbCollectionIden, Expr, ExprOp, Filter, FilterExpr, KvStore, PageData, Query,
    Sort, query::FilterType,
};
use crate::{ActError, Result};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value as JsonValue;
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    fmt::Debug,
    marker::PhantomData,
    sync::Arc,
};

pub struct KvCollection<T> {
    prefix: String,
    kv: Arc<dyn KvStore>,
    _t: PhantomData<T>,
}

impl<T> KvCollection<T> {
    pub fn new(prefix: &str, kv: Arc<dyn KvStore>) -> Self {
        Self {
            prefix: prefix.to_string(),
            kv,
            _t: PhantomData,
        }
    }

    fn data_key(&self, id: &str) -> String {
        format!("{}_id_{}", self.prefix, id)
    }

    fn index_keys(&self, json: &JsonValue, id: &str) -> Vec<String>
    where
        T: DbCollectionIden,
    {
        let fields = T::indexed_fields();
        if fields.is_empty() {
            return Vec::new();
        }
        let mut keys = Vec::with_capacity(fields.len());
        for field in fields {
            if let Some(val) = json.get(field) {
                let val_str = json_value_to_key_str(val);
                keys.push(format!("{}_{}_{}_{}", self.prefix, field, val_str, id));
            }
        }
        keys
    }

    fn read_json(&self, id: &str) -> Result<Option<JsonValue>> {
        let key = self.data_key(id);
        self.kv
            .get(&key)?
            .map(|data| serde_json::from_slice(&data).map_err(map_db_err))
            .transpose()
    }
}

fn json_value_to_key_str(v: &JsonValue) -> String {
    match v {
        JsonValue::String(s) => s.clone(),
        other => other.to_string(),
    }
}

impl<T> DbCollection for KvCollection<T>
where
    T: DbCollectionIden + Serialize + DeserializeOwned + Send + Sync + Clone + Debug + 'static,
{
    type Item = T;

    fn exists(&self, id: &str) -> crate::Result<bool> {
        let key = self.data_key(id);
        self.kv.get(&key).map(|v| v.is_some())
    }

    fn find(&self, id: &str) -> crate::Result<Self::Item> {
        let key = self.data_key(id);
        self.kv
            .get(&key)?
            .map(|data| serde_json::from_slice(&data).map_err(map_db_err))
            .ok_or(ActError::Store(format!(
                "cannot find {} by '{}'",
                self.prefix, id
            )))?
    }

    fn query(&self, q: &Query) -> crate::Result<PageData<Self::Item>> {
        // first, query all indexded data by filter
        let indexed = T::indexed_fields();
        let scan_prefix = if let Some(cond) = &q.filter {
            find_index_hint(cond, indexed).map_or_else(
                || format!("{}_id_", self.prefix),
                |(field, value)| {
                    format!("{}_{}_{}", self.prefix, field, json_value_to_key_str(value))
                },
            )
        } else {
            format!("{}_id_", self.prefix)
        };

        let all = self.kv.scan_prefix(&scan_prefix)?;

        // Deserialize into (key, doc_map) pairs
        let docs: Vec<(String, HashMap<String, JsonValue>)> = all
            .iter()
            .filter_map(|(key, bytes)| {
                serde_json::from_slice::<JsonValue>(bytes)
                    .ok()
                    .and_then(|v| {
                        v.as_object().map(|obj| {
                            let map: HashMap<String, JsonValue> =
                                obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                            (key.clone(), map)
                        })
                    })
            })
            .collect();

        // Apply filter
        let filtered: Vec<&HashMap<String, JsonValue>> = if let Some(cond) = &q.filter {
            let refs: Vec<(&String, &HashMap<String, JsonValue>)> =
                docs.iter().map(|(k, v)| (k, v)).collect();
            let matching_keys = cond.calc(&self.prefix, &refs)?;
            docs.iter()
                .filter_map(|(k, v)| {
                    if matching_keys.contains(k.as_bytes()) {
                        Some(v)
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            docs.iter().map(|(_, v)| v).collect()
        };

        // Sort
        let mut sorted: Vec<&HashMap<String, JsonValue>> = filtered;
        if !q.get_order_by().is_empty() {
            sorted.sort_by(|a, b| {
                let mut ret = Ordering::Equal;
                for ob in q.get_order_by() {
                    let cmp = a
                        .get(&ob.field)
                        .unwrap()
                        .to_string()
                        .cmp(&b.get(&ob.field).unwrap().to_string());
                    match ob.order {
                        Sort::Asc => ret = ret.then(cmp),
                        Sort::Desc => ret = ret.then(cmp.reverse()),
                    }
                }
                ret
            });
        }

        let count = sorted.len();
        let page_count = count.div_ceil(q.limit);
        let page_num = q.offset.checked_div(q.limit).map_or(1, |n| n + 1);

        let rows: Vec<T> = sorted
            .iter()
            .skip(q.offset)
            .take(q.limit)
            .map(|row| map_to_model(row))
            .collect::<Result<Vec<T>>>()?;

        Ok(PageData {
            count,
            page_size: q.limit,
            page_num,
            page_count,
            rows,
        })
    }

    fn create(&self, data: &Self::Item) -> crate::Result<bool> {
        let json = serde_json::to_value(data).map_err(map_db_err)?;
        let id = extract_id(&json)?;
        let bytes = serde_json::to_vec(&json).map_err(map_db_err)?;

        // Write data entry
        self.kv.put(&self.data_key(&id), bytes.clone())?;

        // Write index entries
        for idx_key in self.index_keys(&json, &id) {
            self.kv.put(&idx_key, bytes.clone())?;
        }

        Ok(true)
    }

    fn update(&self, data: &Self::Item) -> crate::Result<bool> {
        let new_json = serde_json::to_value(data).map_err(map_db_err)?;
        let id = extract_id(&new_json)?;
        let new_bytes = serde_json::to_vec(&new_json).map_err(map_db_err)?;

        // Delete old index entries
        if let Some(old_json) = self.read_json(&id)? {
            for idx_key in self.index_keys(&old_json, &id) {
                self.kv.delete(&idx_key)?;
            }
        }

        // Write data entry
        self.kv.put(&self.data_key(&id), new_bytes.clone())?;

        // Write new index entries
        for idx_key in self.index_keys(&new_json, &id) {
            self.kv.put(&idx_key, new_bytes.clone())?;
        }

        Ok(true)
    }

    fn delete(&self, id: &str) -> crate::Result<bool> {
        // Remove index entries
        if let Some(old_json) = self.read_json(id)? {
            for idx_key in self.index_keys(&old_json, id) {
                self.kv.delete(&idx_key)?;
            }
        }

        // Remove data entry
        self.kv.delete(&self.data_key(id))?;
        Ok(true)
    }
}

fn extract_id(json: &JsonValue) -> crate::Result<String> {
    json.get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| ActError::Store("missing id field".to_string()))
}

fn map_to_model<T: DeserializeOwned>(map: &HashMap<String, JsonValue>) -> crate::Result<T> {
    let mut obj = serde_json::Map::new();
    for (k, v) in map {
        obj.insert(k.to_string(), v.clone());
    }
    serde_json::from_value(JsonValue::Object(obj)).map_err(map_db_err)
}

/// Walk the filter tree looking for an EQ condition on an indexed field.
/// Only returns a hint for AND-type filters (a single index prefix cannot satisfy OR).
fn find_index_hint<'a>(filter: &'a Filter, indexed: &[&str]) -> Option<(&'a str, &'a JsonValue)> {
    if filter.r#type != FilterType::And {
        return None;
    }
    for expr in &filter.exprs {
        match expr {
            FilterExpr::Expr(e) if e.op == ExprOp::EQ && indexed.contains(&e.key.as_str()) => {
                return Some((&e.key, &e.value));
            }
            FilterExpr::Filter(sub) => {
                if let Some(hint) = find_index_hint(sub, indexed) {
                    return Some(hint);
                }
            }
            _ => {}
        }
    }
    None
}

impl Filter {
    pub fn calc(
        &self,
        name: &str,
        iters: &[(&String, &HashMap<String, serde_json::Value>)],
    ) -> crate::Result<HashSet<Box<[u8]>>> {
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
    ) -> crate::Result<HashSet<Box<[u8]>>> {
        let get_expr_ret = |expr: &Expr| -> crate::Result<HashSet<Box<[u8]>>> {
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
