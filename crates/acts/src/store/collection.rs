use super::map_db_err;
use crate::store::{
    DbCollection, DbCollectionIden, Expr, ExprOp, Filter, FilterExpr, KvStore, PageData, Query,
    Sort, query::FilterType,
};
use crate::utils::consts::KEY_SEP;
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
        format!("{}{}id{}{}", self.prefix, KEY_SEP, KEY_SEP, id)
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
                keys.push(format!(
                    "{}{}{}{}{}{}{}",
                    self.prefix, KEY_SEP, field, KEY_SEP, val_str, KEY_SEP, id
                ));
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

    /// Compute the set of IDs matching a single expression.
    fn expr_ids(&self, expr: &Expr, indexed: &[&str], is_rev: bool) -> Result<HashSet<String>> {
        if expr.op == ExprOp::EQ && indexed.contains(&expr.key.as_str()) {
            // Use index scan: keys are {prefix}-{field}-{value}-{id}
            let value_str = json_value_to_key_str(&expr.value);
            let scan_key = format!(
                "{}{}{}{}{}{}",
                self.prefix, KEY_SEP, expr.key, KEY_SEP, value_str, KEY_SEP
            );
            let entries = self.kv.scan_prefix(&scan_key, is_rev)?;
            let ids: HashSet<String> = entries
                .iter()
                .filter_map(|(key, _)| {
                    // Key format: {prefix}-{field}-{value}-{id}
                    key.strip_prefix(&scan_key).map(|s| s.to_string())
                })
                .collect();
            Ok(ids)
        } else {
            // Scan all data entries and filter in-memory
            let scan_key = format!("{}{}id{}", self.prefix, KEY_SEP, KEY_SEP);
            let entries = self.kv.scan_prefix(&scan_key, is_rev)?;
            let ids: HashSet<String> = entries
                .iter()
                .filter_map(|(_, bytes)| {
                    let v: JsonValue = serde_json::from_slice(bytes).ok()?;
                    let id = v.get("id")?.as_str()?.to_string();
                    if let Some(field_val) = v.get(&expr.key) {
                        if expr.op(field_val, &expr.value) {
                            return Some(id);
                        }
                    }
                    None
                })
                .collect();
            Ok(ids)
        }
    }

    /// Walk a single FilterExpr node and return matching IDs.
    fn filter_expr_ids(
        &self,
        filter_expr: &FilterExpr,
        indexed: &[&str],
        is_rev: bool,
    ) -> Result<HashSet<String>> {
        match filter_expr {
            FilterExpr::Expr(expr) => self.expr_ids(expr, indexed, is_rev),
            FilterExpr::Filter(filter) => self.filter_ids(filter, indexed, is_rev),
        }
    }

    /// Walk the filter tree and combine ID sets using AND/OR.
    fn filter_ids(
        &self,
        filter: &Filter,
        indexed: &[&str],
        is_rev: bool,
    ) -> Result<HashSet<String>> {
        let mut result: Option<HashSet<String>> = None;
        for cond in &filter.exprs {
            let ids = self.filter_expr_ids(cond, indexed, is_rev)?;
            result = Some(match result {
                None => ids,
                Some(existing) => match filter.r#type {
                    FilterType::And => existing.intersection(&ids).cloned().collect(),
                    FilterType::Or => existing.union(&ids).cloned().collect(),
                },
            });
        }
        Ok(result.unwrap_or_default())
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
        let indexed = T::indexed_fields();

        // Determine is_rev from the first order_by key direction
        let is_rev = q
            .get_order_by()
            .first()
            .map(|ob| ob.order == Sort::Desc)
            .unwrap_or(false);

        // Step 1 & 2: Compute matching ID set from filter and combine with AND/OR
        let id_set: HashSet<String> = if let Some(filter) = &q.filter {
            self.filter_ids(filter, indexed, is_rev)?
        } else {
            // No filter – scan all data entries to collect all IDs
            let scan_key = format!("{}{}id{}", self.prefix, KEY_SEP, KEY_SEP);
            let entries = self.kv.scan_prefix(&scan_key, is_rev)?;
            entries
                .iter()
                .filter_map(|(_, bytes)| {
                    let v: JsonValue = serde_json::from_slice(bytes).ok()?;
                    v.get("id")?.as_str().map(|s| s.to_string())
                })
                .collect()
        };

        // Step 3: Sort the IDs and apply pagination
        let mut ids: Vec<String> = id_set.into_iter().collect();
        if is_rev {
            ids.sort_by(|a, b| b.cmp(a));
        } else {
            ids.sort();
        }

        let count = ids.len();
        let page_ids: Vec<String> = ids.into_iter().skip(q.offset).take(q.limit).collect();

        // Step 4: Fetch full data for paginated IDs and sort by order_by
        let mut docs: Vec<HashMap<String, JsonValue>> = Vec::with_capacity(page_ids.len());
        for id in &page_ids {
            if let Some(json) = self.read_json(id)? {
                if let Some(obj) = json.as_object() {
                    let map: HashMap<String, JsonValue> =
                        obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                    docs.push(map);
                }
            }
        }

        if !q.get_order_by().is_empty() {
            docs.sort_by(|a, b| {
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

        let page_count = count.div_ceil(q.limit);
        let page_num = q.offset.checked_div(q.limit).map_or(1, |n| n + 1);

        let rows: Vec<T> = docs
            .iter()
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
            self.kv.put(&idx_key, vec![])?;
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
