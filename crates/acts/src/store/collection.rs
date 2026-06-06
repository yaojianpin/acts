use super::map_db_err;
use crate::store::{
    DbCollection, DbCollectionIden, Expr, ExprOp, Filter, FilterExpr, KvStore, OrderBy, PageData,
    Query, ScanOperation, ScanOptions, Sort, query::FilterType,
};
use crate::utils::consts::KEY_SEP;
use crate::{ActError, Result};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value as JsonValue;
use std::{cmp::Ordering, collections::HashSet, fmt::Debug, marker::PhantomData, sync::Arc};

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
    fn expr_ids(
        &self,
        expr: &Expr,
        indexed: &[&str],
        order_by: &[OrderBy],
    ) -> Result<HashSet<String>> {
        // Match uses contains() for substring matching, which can't be served
        // by index prefix scan (starts_with). Fall through to non-indexed path.
        if indexed.contains(&expr.key.as_str()) && expr.op != ExprOp::Match {
            // Determine scan direction from order_by for this expression's field
            let is_rev = order_by
                .iter()
                .find(|ob| ob.field == expr.key)
                .map(|ob| ob.order == Sort::Desc)
                .unwrap_or(false);

            // field_prefix bounds the scan to this field: {prefix}-{field}-
            let field_prefix = format!("{}{}{}{}", self.prefix, KEY_SEP, expr.key, KEY_SEP);
            let value_str = json_value_to_key_str(&expr.value);
            let value_key = format!(
                "{}{}{}{}{}{}",
                self.prefix, KEY_SEP, expr.key, KEY_SEP, value_str, KEY_SEP
            );

            let scan_op = match expr.op {
                ExprOp::EQ => ScanOperation::Eq,
                ExprOp::NE => ScanOperation::Ne,
                ExprOp::GT => ScanOperation::Gt,
                ExprOp::GE => ScanOperation::Ge,
                ExprOp::LT => ScanOperation::Lt,
                ExprOp::LE => ScanOperation::Le,
                ExprOp::Match => ScanOperation::Match,
                ExprOp::Between => {
                    let empty = vec![];
                    let arr = expr.value.as_array().unwrap_or(&empty);
                    let from = if arr.len() > 1 {
                        json_value_to_key_str(&arr[0])
                    } else {
                        "".to_string()
                    };
                    let to = if arr.len() > 1 {
                        json_value_to_key_str(&arr[1])
                    } else {
                        "".to_string()
                    };
                    ScanOperation::InclusiveRange { from, to }
                }
                ExprOp::In => {
                    let empty = vec![];
                    let arr = expr.value.as_array().unwrap_or(&empty);
                    let values: Vec<String> = arr
                        .iter()
                        .map(|val| {
                            let v_str = json_value_to_key_str(val);
                            format!(
                                "{}{}{}{}{}{}",
                                self.prefix, KEY_SEP, expr.key, KEY_SEP, v_str, KEY_SEP
                            )
                        })
                        .collect();
                    ScanOperation::In { values }
                }
            };

            let scan_key = match expr.op {
                // For range/In ops, `key` equals `prefix` (field-level prefix)
                ExprOp::Between | ExprOp::In => field_prefix.clone(),
                _ => value_key.clone(),
            };

            let options = ScanOptions::new(scan_op, field_prefix.clone(), is_rev);
            let entries = self.kv.scan_prefix(&scan_key, options)?;

            let ids: HashSet<String> = match expr.op {
                // For Eq/Match, returned keys all start with value_key
                ExprOp::EQ | ExprOp::Match => entries
                    .iter()
                    .filter_map(|(key, _)| key.strip_prefix(&value_key).map(|s| s.to_string()))
                    .collect(),
                // For other ops, extract ID from the index key by skipping
                // past the field_prefix and the value segment
                _ => entries
                    .iter()
                    .filter_map(|(key, _)| {
                        let rest = key.strip_prefix(&field_prefix)?;
                        let sep_pos = rest.rfind(KEY_SEP)?;
                        Some(rest[sep_pos + KEY_SEP.len()..].to_string())
                    })
                    .collect(),
            };
            Ok(ids)
        } else {
            // Fallback: scan all data entries and filter in-memory
            let scan_key = format!("{}{}id{}", self.prefix, KEY_SEP, KEY_SEP);
            let options = ScanOptions::new(ScanOperation::Eq, scan_key.clone(), false);
            let entries = self.kv.scan_prefix(&scan_key, options)?;
            let ids: HashSet<String> = entries
                .iter()
                .filter_map(|(_, bytes)| {
                    let v: JsonValue = serde_json::from_slice(bytes).ok()?;
                    let id = v.get("id")?.as_str()?.to_string();
                    if let Some(field_val) = v.get(&expr.key)
                        && expr.op(field_val, &expr.value)
                    {
                        return Some(id);
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
        order_by: &[OrderBy],
    ) -> Result<HashSet<String>> {
        match filter_expr {
            FilterExpr::Expr(expr) => self.expr_ids(expr, indexed, order_by),
            FilterExpr::Filter(filter) => self.filter_ids(filter, indexed, order_by),
        }
    }

    /// Walk the filter tree and combine ID sets using AND/OR.
    fn filter_ids(
        &self,
        filter: &Filter,
        indexed: &[&str],
        order_by: &[OrderBy],
    ) -> Result<HashSet<String>> {
        let mut result: Option<HashSet<String>> = None;
        for cond in &filter.exprs {
            let ids = self.filter_expr_ids(cond, indexed, order_by)?;
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

/// Convert a JSON value to a string suitable for use as an index-key segment.
///
/// Integers (i64, u64) are zero-padded to 20 digits so that lexicographic
/// ordering matches numeric ordering (otherwise "10" < "5").
/// Compare two JsonValues for ordering.
///
/// Numbers are compared as f64; everything else is compared as a string.
fn cmp_json_val(a: &JsonValue, b: &JsonValue) -> Ordering {
    if let (JsonValue::Number(na), JsonValue::Number(nb)) = (a, b) {
        let fa = na.as_f64().unwrap_or_default();
        let fb = nb.as_f64().unwrap_or_default();
        if fa < fb {
            Ordering::Less
        } else if fa > fb {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    } else {
        a.to_string().cmp(&b.to_string())
    }
}

/// Encode a string so it contains only characters valid for all KV store keys
/// and safe for SQL LIKE patterns (i.e. no `%`, `_`, or `\`).
///
/// Characters in `[a-zA-Z0-9-]` pass through unchanged. Every other character
/// — including `%`, `_`, `\`, `|`, `=`, `.` — is encoded as `=XX` (2-digit
/// uppercase hex for code points 0–255, 6-digit for code points above 255).
/// The `=` escape-prefix is itself valid in NATS KV keys (the strictest
/// backend), and the encoding is applied identically during key creation and
/// query scan-key construction so that lookups always match.
fn encode_key_str(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' => result.push(c),
            other => {
                result.push('=');
                let code = other as u32;
                if code <= 0xFF {
                    result.push_str(&format!("{:02X}", code));
                } else {
                    result.push_str(&format!("{:06X}", code));
                }
            }
        }
    }
    result
}

fn json_value_to_key_str(v: &JsonValue) -> String {
    match v {
        JsonValue::String(s) => encode_key_str(s),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                format!("{:020}", i)
            } else if let Some(u) = n.as_u64() {
                format!("{:020}", u)
            } else {
                n.to_string()
            }
        }
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
        let data = self.kv.get(&key)?.ok_or(ActError::Store(format!(
            "cannot find {} by '{}'",
            self.prefix, id
        )))?;
        let json: JsonValue = serde_json::from_slice(&data).map_err(map_db_err)?;
        T::upcast(json)
    }

    fn query(&self, q: &Query) -> crate::Result<PageData<Self::Item>> {
        let indexed = T::indexed_fields();

        // Determine global is_rev for no-filter fallback and final ID sort
        let is_rev = q
            .get_order_by()
            .iter()
            .find(|ob| indexed.contains(&ob.field.as_str()))
            .map(|ob| ob.order == Sort::Desc)
            .unwrap_or(false);

        // Step 1 & 2: Compute matching ID set from filter and combine with AND/OR
        let id_set: HashSet<String> = if let Some(filter) = &q.filter {
            self.filter_ids(filter, indexed, q.get_order_by())?
        } else {
            // No filter – scan all data entries to collect all IDs
            let scan_key = format!("{}{}id{}", self.prefix, KEY_SEP, KEY_SEP);
            let options = ScanOptions::new(ScanOperation::Eq, scan_key.clone(), is_rev);
            let entries = self.kv.scan_prefix(&scan_key, options)?;
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
        let mut docs: Vec<JsonValue> = Vec::with_capacity(page_ids.len());
        for id in &page_ids {
            if let Some(json) = self.read_json(id)? {
                docs.push(json);
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
            .map(|row| T::upcast(row.clone()))
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
                // Extract raw strings for comparison (not JSON-encoded to_string,
                // which would escape \ as \\ and cause false negatives)
                let l_str: String = match l {
                    JsonValue::String(v) => v.clone(),
                    other => other.to_string(),
                };
                let r_str: String = match r {
                    JsonValue::String(v) => v.clone(),
                    other => other.to_string(),
                };
                l_str.contains(&r_str)
            }
            ExprOp::Between => {
                let arr = match r.as_array() {
                    Some(a) if a.len() >= 2 => a,
                    _ => return false,
                };
                cmp_json_val(l, &arr[0]) != Ordering::Less
                    && cmp_json_val(l, &arr[1]) != Ordering::Greater
            }
            ExprOp::In => {
                let arr = match r.as_array() {
                    Some(a) => a,
                    None => return false,
                };
                arr.iter().any(|v| cmp_json_val(l, v) == Ordering::Equal)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{encode_key_str, json_value_to_key_str};
    use crate::store::Expr;
    use serde_json::json;

    #[test]
    fn encode_key_str_passthrough() {
        // Characters in the safe set pass through unchanged
        assert_eq!(encode_key_str("hello"), "hello");
        assert_eq!(encode_key_str("abcABC123"), "abcABC123");
        assert_eq!(encode_key_str("hello-world"), "hello-world");
        assert_eq!(encode_key_str("with_underscore"), "with=5Funderscore");
        assert_eq!(encode_key_str(""), "");
    }

    #[test]
    fn encode_key_str_percent() {
        assert_eq!(encode_key_str("50%off"), "50=25off");
    }

    #[test]
    fn encode_key_str_pipe() {
        assert_eq!(encode_key_str("a|b|c"), "a=7Cb=7Cc");
    }

    #[test]
    fn encode_key_str_backslash() {
        assert_eq!(encode_key_str(r"a\b"), "a=5Cb");
    }

    #[test]
    fn encode_key_str_equals() {
        // = itself is encoded so it never appears un-escaped in the output
        assert_eq!(encode_key_str("a=b"), "a=3Db");
    }

    #[test]
    fn encode_key_str_dot() {
        assert_eq!(encode_key_str("file.txt"), "file=2Etxt");
    }

    #[test]
    fn encode_key_str_mixed_special() {
        assert_eq!(encode_key_str("a%b|c\\d"), "a=25b=7Cc=5Cd");
    }

    #[test]
    fn encode_key_str_emoji() {
        // Non-BMP character encoded with 6-digit hex
        let s = encode_key_str("hi😀");
        assert!(s.starts_with("hi="));
        assert!(s.len() > 4);
    }

    #[test]
    fn json_value_to_key_str_string() {
        assert_eq!(json_value_to_key_str(&json!("hello")), "hello");
    }

    #[test]
    fn json_value_to_key_str_string_with_special() {
        // Special characters are encoded via encode_key_str
        assert_eq!(json_value_to_key_str(&json!("a%b")), "a=25b");
    }

    #[test]
    fn json_value_to_key_str_i64_zero_pads() {
        assert_eq!(json_value_to_key_str(&json!(5)), "00000000000000000005");
        assert_eq!(json_value_to_key_str(&json!(10)), "00000000000000000010");
        assert_eq!(json_value_to_key_str(&json!(100)), "00000000000000000100");
    }

    #[test]
    fn json_value_to_key_str_i64_negative() {
        assert_eq!(json_value_to_key_str(&json!(-5)), "-0000000000000000005");
    }

    #[test]
    fn json_value_to_key_str_u64_zero_pads() {
        let big: u64 = u64::MAX;
        assert_eq!(json_value_to_key_str(&json!(big)), "18446744073709551615");
    }

    #[test]
    fn json_value_to_key_str_lexicographic_order() {
        // Verify that zero-padded integers sort correctly lexicographically:
        // after zero-padding, "000000000...5" < "000000000...10"
        let key1 = json_value_to_key_str(&json!(1));
        let key2 = json_value_to_key_str(&json!(2));
        let key5 = json_value_to_key_str(&json!(5));
        let key10 = json_value_to_key_str(&json!(10));
        let key100 = json_value_to_key_str(&json!(100));

        let mut sorted = vec![&key10, &key100, &key1, &key5, &key2];
        sorted.sort();
        assert_eq!(sorted, vec![&key1, &key2, &key5, &key10, &key100]);
    }

    #[test]
    fn json_value_to_key_str_float_no_padding() {
        // Floats are not padded — they can't be ordered lexicographically anyway
        let v = json!(2.71);
        let s = json_value_to_key_str(&v);
        assert!(s.contains("2.71"));
    }

    #[test]
    fn json_value_to_key_str_bool() {
        assert_eq!(json_value_to_key_str(&json!(true)), "true");
        assert_eq!(json_value_to_key_str(&json!(false)), "false");
    }

    #[test]
    fn json_value_to_key_str_null() {
        assert_eq!(json_value_to_key_str(&json!(null)), "null");
    }

    // ========== Expr::op() Between / In / cmp_json_val tests ==========

    #[test]
    fn store_expr_op_between_numbers_inside() {
        let expr = Expr::between("field", 10, 20);
        assert!(expr.op(&json!(10), &json!([10, 20])));
        assert!(expr.op(&json!(15), &json!([10, 20])));
        assert!(expr.op(&json!(20), &json!([10, 20])));
    }

    #[test]
    fn store_expr_op_between_numbers_outside() {
        let expr = Expr::between("field", 10, 20);
        assert!(!expr.op(&json!(9), &json!([10, 20])));
        assert!(!expr.op(&json!(21), &json!([10, 20])));
        assert!(!expr.op(&json!(100), &json!([10, 20])));
    }

    #[test]
    fn store_expr_op_between_strings() {
        let expr = Expr::between("field", "b", "d");
        assert!(!expr.op(&json!("a"), &json!(["b", "d"])));
        assert!(expr.op(&json!("b"), &json!(["b", "d"])));
        assert!(expr.op(&json!("c"), &json!(["b", "d"])));
        assert!(expr.op(&json!("d"), &json!(["b", "d"])));
        assert!(!expr.op(&json!("e"), &json!(["b", "d"])));
    }

    #[test]
    fn store_expr_op_between_invalid_array() {
        let expr = Expr::between("field", 1, 9);
        // Not an array — returns false
        assert!(!expr.op(&json!(5), &json!("not_array")));
        // Array with single element — returns false
        assert!(!expr.op(&json!(5), &json!([1])));
        // Empty array — returns false
        assert!(!expr.op(&json!(5), &json!([])));
    }

    #[test]
    fn store_expr_op_between_float() {
        let expr = Expr::between("field", 1.5, 3.5);
        assert!(!expr.op(&json!(1.0), &json!([1.5, 3.5])));
        assert!(expr.op(&json!(1.5), &json!([1.5, 3.5])));
        assert!(expr.op(&json!(2.0), &json!([1.5, 3.5])));
        assert!(expr.op(&json!(3.5), &json!([1.5, 3.5])));
        assert!(!expr.op(&json!(4.0), &json!([1.5, 3.5])));
    }

    #[test]
    fn store_expr_op_in_numbers() {
        let expr = Expr::r#in("field", vec![1, 3, 5]);
        assert!(expr.op(&json!(1), &json!([1, 3, 5])));
        assert!(expr.op(&json!(3), &json!([1, 3, 5])));
        assert!(expr.op(&json!(5), &json!([1, 3, 5])));
        assert!(!expr.op(&json!(0), &json!([1, 3, 5])));
        assert!(!expr.op(&json!(2), &json!([1, 3, 5])));
        assert!(!expr.op(&json!(6), &json!([1, 3, 5])));
    }

    #[test]
    fn store_expr_op_in_strings() {
        let expr = Expr::r#in("field", vec!["running", "completed"]);
        assert!(expr.op(&json!("running"), &json!(["running", "completed"])));
        assert!(expr.op(&json!("completed"), &json!(["running", "completed"])));
        assert!(!expr.op(&json!("pending"), &json!(["running", "completed"])));
        assert!(!expr.op(&json!("none"), &json!(["running", "completed"])));
    }

    #[test]
    fn store_expr_op_in_invalid() {
        let expr = Expr::r#in("field", vec![1, 2]);
        // Not an array — returns false
        assert!(!expr.op(&json!(1), &json!("not_array")));
        // Null — returns false
        assert!(!expr.op(&json!(1), &json!(null)));
    }

    #[test]
    fn store_expr_op_in_empty() {
        let expr = Expr::r#in("field", Vec::<i32>::new());
        // No values to match, always false
        assert!(!expr.op(&json!(1), &json!([])));
        assert!(!expr.op(&json!("a"), &json!([])));
    }

    // ========== cmp_json_val comparison tests ==========

    #[test]
    fn store_cmp_json_val_numbers() {
        use super::cmp_json_val;
        use std::cmp::Ordering;
        assert_eq!(cmp_json_val(&json!(10), &json!(5)), Ordering::Greater);
        assert_eq!(cmp_json_val(&json!(5), &json!(10)), Ordering::Less);
        assert_eq!(cmp_json_val(&json!(5), &json!(5)), Ordering::Equal);
    }

    #[test]
    fn store_cmp_json_val_strings() {
        use super::cmp_json_val;
        use std::cmp::Ordering;
        assert_eq!(cmp_json_val(&json!("abc"), &json!("abc")), Ordering::Equal);
        assert_eq!(cmp_json_val(&json!("abc"), &json!("def")), Ordering::Less);
        assert_eq!(
            cmp_json_val(&json!("def"), &json!("abc")),
            Ordering::Greater
        );
    }

    #[test]
    fn store_cmp_json_val_mixed_types() {
        use super::cmp_json_val;
        use std::cmp::Ordering;
        // number vs string — compared via to_string()
        // json!(10).to_string() = "10", json!("5").to_string() = "\"5\""
        // "10" > "\"5\"" because '1' (49) > '"' (34)
        let result = cmp_json_val(&json!(10), &json!("5"));
        assert_eq!(result, Ordering::Greater); // "10" > "\"5\"" lexicographically
    }

    #[test]
    fn store_cmp_json_val_floats() {
        use super::cmp_json_val;
        use std::cmp::Ordering;
        assert_eq!(cmp_json_val(&json!(1.5), &json!(1.5)), Ordering::Equal);
        assert_eq!(cmp_json_val(&json!(1.5), &json!(2.0)), Ordering::Less);
        assert_eq!(cmp_json_val(&json!(3.0), &json!(2.5)), Ordering::Greater);
    }

    // ========== Expr::op() NE / LT / LE / GT / GE / Match tests ==========

    #[test]
    fn store_expr_op_ne_numbers() {
        let expr = Expr::ne("field", 10);
        assert!(!expr.op(&json!(10), &json!(10)));
        assert!(expr.op(&json!(5), &json!(10)));
        assert!(expr.op(&json!(20), &json!(10)));
    }

    #[test]
    fn store_expr_op_ne_strings() {
        let expr = Expr::ne("field", "hello");
        assert!(!expr.op(&json!("hello"), &json!("hello")));
        assert!(expr.op(&json!("world"), &json!("hello")));
        assert!(expr.op(&json!(""), &json!("hello")));
    }

    #[test]
    fn store_expr_op_ne_mixed_types() {
        let expr = Expr::ne("field", 10);
        // NE returns true when types differ (l != r)
        assert!(expr.op(&json!("10"), &json!(10)));
    }

    #[test]
    fn store_expr_op_lt_numbers() {
        let expr = Expr::lt("field", 10);
        assert!(expr.op(&json!(5), &json!(10)));
        assert!(!expr.op(&json!(10), &json!(10)));
        assert!(!expr.op(&json!(15), &json!(10)));
    }

    #[test]
    fn store_expr_op_lt_non_number_returns_false() {
        let expr = Expr::lt("field", 10);
        // LT only works for numbers; non-numbers always return false
        assert!(!expr.op(&json!("5"), &json!(10)));
        assert!(!expr.op(&json!(null), &json!(10)));
    }

    #[test]
    fn store_expr_op_le_numbers() {
        let expr = Expr::le("field", 10);
        assert!(expr.op(&json!(5), &json!(10)));
        assert!(expr.op(&json!(10), &json!(10)));
        assert!(!expr.op(&json!(15), &json!(10)));
    }

    #[test]
    fn store_expr_op_le_non_number_returns_false() {
        let expr = Expr::le("field", 10);
        assert!(!expr.op(&json!("5"), &json!(10)));
    }

    #[test]
    fn store_expr_op_gt_numbers() {
        let expr = Expr::gt("field", 10);
        assert!(!expr.op(&json!(5), &json!(10)));
        assert!(!expr.op(&json!(10), &json!(10)));
        assert!(expr.op(&json!(15), &json!(10)));
    }

    #[test]
    fn store_expr_op_gt_non_number_returns_false() {
        let expr = Expr::gt("field", 10);
        assert!(!expr.op(&json!("15"), &json!(10)));
    }

    #[test]
    fn store_expr_op_ge_numbers() {
        let expr = Expr::ge("field", 10);
        assert!(!expr.op(&json!(5), &json!(10)));
        assert!(expr.op(&json!(10), &json!(10)));
        assert!(expr.op(&json!(15), &json!(10)));
    }

    #[test]
    fn store_expr_op_ge_non_number_returns_false() {
        let expr = Expr::ge("field", 10);
        assert!(!expr.op(&json!("15"), &json!(10)));
    }

    #[test]
    fn store_expr_op_match_contains() {
        let expr = Expr::matches("field", "ello");
        assert!(expr.op(&json!("hello"), &json!("ello")));
        assert!(!expr.op(&json!("hello"), &json!("xyz")));
    }

    #[test]
    fn store_expr_op_match_number_is_substring_of_to_string() {
        // Match converts both sides to string and checks contains
        let expr = Expr::matches("field", "2");
        assert!(expr.op(&json!(12), &json!("2")));
        assert!(!expr.op(&json!(10), &json!("2")));
    }
}
