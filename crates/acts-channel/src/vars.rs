use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::ops::{Deref, DerefMut};

#[derive(Default, Clone)]
pub struct Vars {
    inner: Map<String, Value>,
}

#[allow(dead_code)]
pub struct Iter<'a> {
    iter: serde_json::map::Iter<'a>,
}

pub struct IterMut<'a> {
    iter: serde_json::map::IterMut<'a>,
}

impl Serialize for Vars {
    fn serialize<S>(&self, serializer: S) -> core::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.inner.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Vars {
    fn deserialize<D>(deserializer: D) -> core::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        core::result::Result::Ok(Self {
            inner: serde_json::Map::deserialize(deserializer)?,
        })
    }
}

impl Deref for Vars {
    type Target = Map<String, Value>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for Vars {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl FromIterator<(String, Value)> for Vars {
    fn from_iter<T: IntoIterator<Item = (String, Value)>>(iter: T) -> Self {
        Self {
            inner: Map::from_iter(iter),
        }
    }
}

impl<'a> Iterator for Iter<'a> {
    type Item = (&'a String, &'a Value);

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}

impl<'a> Iterator for IterMut<'a> {
    type Item = (&'a String, &'a mut Value);

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}

impl<'a> IntoIterator for &'a mut Vars {
    type Item = (&'a String, &'a mut Value);
    type IntoIter = IterMut<'a>;
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        IterMut {
            iter: self.inner.iter_mut(),
        }
    }
}

impl IntoIterator for &Vars {
    type Item = (String, Value);
    type IntoIter = serde_json::map::IntoIter;
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.inner.clone().into_iter()
    }
}

impl std::fmt::Debug for Vars {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = serde_json::to_string(&self.inner).map_err(|_| std::fmt::Error)?;
        f.write_str(&text)
    }
}

impl std::fmt::Display for Vars {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = serde_json::to_string(&self.inner).map_err(|_| std::fmt::Error)?;
        f.write_str(&text)
    }
}

impl From<serde_json::Map<String, Value>> for Vars {
    fn from(value: serde_json::Map<String, Value>) -> Self {
        from_json(&value)
    }
}

impl From<serde_json::Value> for Vars {
    fn from(value: serde_json::Value) -> Self {
        if let serde_json::Value::Object(map) = &value {
            return from_json(map);
        }
        Vars::new()
    }
}

impl From<Vars> for serde_json::Value {
    fn from(val: Vars) -> Self {
        serde_json::Value::Object(val.inner)
    }
}

impl Vars {
    pub fn new() -> Self {
        Self { inner: Map::new() }
    }

    pub fn with<T>(self, name: &str, value: T) -> Self
    where
        T: Serialize,
    {
        let mut vars = self.inner;
        vars.insert(name.to_string(), json!(value));

        Self { inner: vars }
    }

    pub fn set<T>(&mut self, name: &str, value: T)
    where
        T: Serialize + Clone,
    {
        let value = json!(value);
        self.inner
            .entry(name.to_string())
            .and_modify(|v| *v = value.clone())
            .or_insert(value);
    }

    pub fn get<T>(&self, name: &str) -> Option<T>
    where
        T: for<'de> Deserialize<'de> + Clone,
    {
        if let Some(value) = self.inner.get(name)
            && let Ok(value) = serde_json::from_value::<T>(value.clone())
        {
            return Some(value);
        }

        None
    }

    pub fn get_value(&self, name: &str) -> Option<&Value> {
        self.inner.get(name)
    }

    pub fn extend(mut self, vars: &Vars) -> Self {
        self.inner.extend(vars);
        self
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&self.inner).unwrap()
    }

    pub fn value_str(&self, key: &str) -> Option<&str> {
        self.inner.get(key).and_then(|v| v.as_str())
    }

    pub fn value_number(&self, key: &str) -> Option<f64> {
        self.inner.get(key).and_then(|v| v.as_f64())
    }
}

#[allow(unused)]
pub fn from_json(map: &serde_json::Map<String, serde_json::Value>) -> Vars {
    let mut vars = Vars::new();

    for (k, v) in map {
        let value = match v {
            serde_json::Value::Null => Value::Null,
            serde_json::Value::Bool(v) => Value::Bool(*v),
            serde_json::Value::Number(v) => from_json_number(v),
            serde_json::Value::String(v) => Value::String(v.clone()),
            serde_json::Value::Array(v) => from_json_array(v),
            serde_json::Value::Object(v) => from_json_object(v),
        };

        vars.insert(k.to_string(), value);
    }

    vars
}

#[allow(unused)]
fn from_json_array(arr: &Vec<serde_json::Value>) -> Value {
    let mut ret = Vec::new();
    for v in arr {
        let value = match v {
            serde_json::Value::Null => Value::Null,
            serde_json::Value::Bool(v) => Value::Bool(*v),
            serde_json::Value::Number(v) => from_json_number(v),
            serde_json::Value::String(v) => Value::String(v.clone()),
            serde_json::Value::Array(v) => from_json_array(v),
            serde_json::Value::Object(v) => from_json_object(v),
        };
        ret.push(value);
    }

    Value::Array(ret)
}

#[allow(unused)]
fn from_json_object(o: &serde_json::Map<String, serde_json::Value>) -> Value {
    let mut map = serde_json::Map::new();
    for (k, v) in o {
        let value = match v {
            serde_json::Value::Null => Value::Null,
            serde_json::Value::Bool(v) => Value::Bool(*v),
            serde_json::Value::Number(v) => from_json_number(v),
            serde_json::Value::String(v) => Value::String(v.clone()),
            serde_json::Value::Array(v) => from_json_array(v),
            serde_json::Value::Object(v) => from_json_object(v),
        };

        map.insert(k.to_string(), value);
    }

    Value::Object(map)
}

#[allow(unused)]
fn from_json_number(n: &serde_json::Number) -> Value {
    if n.is_i64() {
        Value::Number(serde_json::Number::from(n.as_i64().unwrap()))
    } else if n.is_u64() {
        Value::Number(serde_json::Number::from(n.as_u64().unwrap()))
    } else {
        Value::Number(serde_json::Number::from_f64(n.as_f64().unwrap()).unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::{Iter, IterMut, Vars, from_json, from_json_number};
    use serde_json::{Map, Value, json};

    #[test]
    fn new_default_and_debug_display() {
        let vars = Vars::new();
        assert!(vars.is_empty());
        assert_eq!(format!("{vars:?}"), "{}");
        assert_eq!(format!("{vars}"), "{}");
        assert_eq!(Vars::default().len(), 0);
    }

    #[test]
    fn with_set_get_and_remove() {
        let mut vars = Vars::new().with("a", 1).with("b", "x");
        assert_eq!(vars.get::<i32>("a"), Some(1));
        // a present key of the wrong type deserializes to a miss
        assert_eq!(vars.get::<String>("a"), None);
        assert_eq!(vars.get::<String>("b").as_deref(), Some("x"));
        // a missing key is a miss
        assert_eq!(vars.get::<i32>("missing"), None);
        assert_eq!(vars.get_value("a"), Some(&json!(1)));
        assert_eq!(vars.get_value("missing"), None);

        // set overwrites an existing entry and inserts a new one
        vars.set("a", 2);
        vars.set("c", true);
        assert_eq!(vars.get::<i32>("a"), Some(2));
        assert_eq!(vars.value_number("a"), Some(2.0));
        assert_eq!(vars.value_str("b"), Some("x"));
        assert_eq!(vars.value_str("c"), None);
        assert_eq!(vars.value_number("b"), None);

        assert_eq!(vars.remove("a"), Some(json!(2)));
        assert!(!vars.contains_key("a"));
        assert!(vars.contains_key("c"));
    }

    #[test]
    fn deref_and_deref_mut_reach_the_map() {
        let mut vars = Vars::new();
        // DerefMut
        vars.insert("k".to_string(), json!("v"));
        assert_eq!(vars.len(), 1);
        // Deref
        assert_eq!(vars.keys().next().map(String::as_str), Some("k"));
        assert!(Map::get(&vars, "k").is_some());
        vars.get_mut("k").unwrap().clone_from(&json!("w"));
        assert_eq!(vars.value_str("k"), Some("w"));
    }

    #[test]
    fn extend_merges_and_overrides() {
        let a = Vars::new().with("a", 1).with("shared", "first");
        let b = Vars::new().with("b", 2).with("shared", "second");
        let merged = a.extend(&b);
        assert_eq!(merged.get::<i32>("a"), Some(1));
        assert_eq!(merged.get::<i32>("b"), Some(2));
        assert_eq!(merged.get::<String>("shared").as_deref(), Some("second"));
    }

    #[test]
    fn to_bytes_is_the_json_object() {
        let vars = Vars::new().with("a", 1);
        let value: Value = serde_json::from_slice(&vars.to_bytes()).unwrap();
        assert_eq!(value, json!({"a": 1}));
    }

    #[test]
    fn serde_roundtrip() {
        let vars = Vars::new()
            .with("a", json!({"b": [1, null, true]}))
            .with("c", "x");
        let text = serde_json::to_string(&vars).unwrap();
        assert_eq!(text, r#"{"a":{"b":[1,null,true]},"c":"x"}"#);
        let back: Vars = serde_json::from_str(&text).unwrap();
        assert_eq!(back.get_value("a"), vars.get_value("a"));
        assert_eq!(back.get::<String>("c").as_deref(), Some("x"));
    }

    #[test]
    fn from_value_and_map() {
        let map = json!({"a": 1, "nested": {"x": [true, null]}});
        let vars = Vars::from(map.clone());
        assert_eq!(vars.get::<i32>("a"), Some(1));
        assert_eq!(vars.get_value("nested"), Some(&json!({"x": [true, null]})));

        let Value::Object(obj) = map else {
            unreachable!("the literal is an object")
        };
        let from_map = Vars::from(obj);
        assert_eq!(from_map.get::<i32>("a"), Some(1));

        // a non-object value has no keys to carry over
        assert!(Vars::from(json!("not an object")).is_empty());
        assert!(Vars::from(Value::Null).is_empty());

        let value: Value = Vars::new().with("a", 1).into();
        assert_eq!(value, json!({"a": 1}));
    }

    #[test]
    fn from_json_covers_every_json_kind() {
        let map = json!({
            "null": null,
            "bool": true,
            "i64": -5,
            "u64": u64::MAX,
            "f64": 1.5,
            "str": "text",
            "arr": [null, false, 1, "s", [1, 2], {"k": 2.5}],
            "obj": {
                "n": null,
                "b": false,
                "i": 1,
                "u": 18446744073709551615u64,
                "f": 0.25,
                "s": "x",
                "a": [1],
                "o": {"deep": true}
            }
        });
        let vars = from_json(map.as_object().unwrap());

        assert_eq!(vars.get_value("null"), Some(&Value::Null));
        assert_eq!(vars.get_value("bool"), Some(&json!(true)));
        assert_eq!(vars.get_value("i64"), Some(&json!(-5)));
        assert_eq!(vars.get_value("u64"), Some(&json!(u64::MAX)));
        assert_eq!(vars.get_value("f64"), Some(&json!(1.5)));
        assert_eq!(vars.get_value("str"), Some(&json!("text")));
        assert_eq!(
            vars.get_value("arr"),
            Some(&json!([null, false, 1, "s", [1, 2], {"k": 2.5}]))
        );
        assert_eq!(
            vars.get_value("obj"),
            Some(&json!({
                "n": null,
                "b": false,
                "i": 1,
                "u": 18446744073709551615u64,
                "f": 0.25,
                "s": "x",
                "a": [1],
                "o": {"deep": true}
            }))
        );
    }

    #[test]
    fn from_json_number_prefers_signed_then_unsigned_then_float() {
        assert_eq!(
            from_json_number(&serde_json::Number::from(-1i64)),
            json!(-1)
        );
        assert_eq!(from_json_number(&serde_json::Number::from(7u64)), json!(7));
        let float = serde_json::Number::from_f64(2.5).unwrap();
        assert_eq!(from_json_number(&float), json!(2.5));
    }

    #[test]
    fn iterators_lend_borrow_and_mutate() {
        let mut vars = Vars::new().with("a", 1).with("b", 2);

        // the shared iterator wrapper
        let mut iter = Iter {
            iter: vars.inner.iter(),
        };
        assert_eq!(
            iter.next().map(|(k, v)| (k.as_str(), v.clone())),
            Some(("a", json!(1)))
        );
        assert_eq!(iter.next().map(|(k, _)| k.as_str()), Some("b"));
        assert_eq!(iter.next(), None);

        // `&Vars` yields owned pairs from a cloned map
        let pairs: Vec<(String, Value)> = (&vars).into_iter().collect();
        assert_eq!(pairs.len(), 2);

        // `&mut Vars` lends mutable values
        for (_, value) in &mut vars {
            *value = json!(0);
        }
        assert_eq!(vars.get::<i32>("a"), Some(0));
        assert_eq!(vars.get::<i32>("b"), Some(0));

        // and the mutable wrapper directly
        let mut iter_mut = IterMut {
            iter: vars.inner.iter_mut(),
        };
        assert!(iter_mut.next().is_some());
        assert!(iter_mut.next().is_some());
        assert!(iter_mut.next().is_none());
    }

    #[test]
    fn from_iterator_collects_pairs() {
        let vars: Vars = vec![("a".to_string(), json!(1)), ("b".to_string(), json!("x"))]
            .into_iter()
            .collect();
        assert_eq!(vars.get::<i32>("a"), Some(1));
        assert_eq!(vars.get::<String>("b").as_deref(), Some("x"));
    }
}
