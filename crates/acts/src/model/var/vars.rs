use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue, json};
use std::ops::{Deref, DerefMut};

use super::Variant;

#[derive(Default, Clone)]
pub struct Vars {
    inner: JsonMap<String, JsonValue>,
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
    type Target = JsonMap<String, JsonValue>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for Vars {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl FromIterator<(String, JsonValue)> for Vars {
    fn from_iter<T: IntoIterator<Item = (String, JsonValue)>>(iter: T) -> Self {
        Self {
            inner: JsonMap::from_iter(iter),
        }
    }
}

impl<'a> Iterator for Iter<'a> {
    type Item = (&'a String, &'a JsonValue);

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}

impl<'a> Iterator for IterMut<'a> {
    type Item = (&'a String, &'a mut JsonValue);

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}

impl<'a> IntoIterator for &'a mut Vars {
    type Item = (&'a String, &'a mut JsonValue);
    type IntoIter = IterMut<'a>;
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        IterMut {
            iter: self.inner.iter_mut(),
        }
    }
}

impl IntoIterator for &Vars {
    type Item = (String, JsonValue);
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

impl From<JsonMap<String, JsonValue>> for Vars {
    fn from(value: JsonMap<String, JsonValue>) -> Self {
        from_json(&value)
    }
}

impl From<JsonValue> for Vars {
    fn from(value: JsonValue) -> Self {
        if let JsonValue::Object(map) = &value {
            return from_json(map);
        }
        Vars::new()
    }
}

impl From<Vars> for JsonValue {
    fn from(val: Vars) -> Self {
        JsonValue::Object(val.inner)
    }
}

impl From<Vec<Variant>> for Vars {
    fn from(val: Vec<Variant>) -> Self {
        let mut vars = Vars::new();
        for var in val {
            vars.set(&var.name, var.value);
        }
        vars
    }
}

impl Vars {
    pub fn new() -> Self {
        Self {
            inner: JsonMap::new(),
        }
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

    pub fn get_value(&self, name: &str) -> Option<&JsonValue> {
        self.inner.get(name)
    }

    pub fn pop<T>(&mut self, name: &str) -> Option<T>
    where
        T: for<'de> Deserialize<'de> + Clone,
    {
        if let Some(value) = self.inner.remove(name)
            && let Ok(value) = serde_json::from_value::<T>(value)
        {
            return Some(value);
        }

        None
    }

    pub fn extend(mut self, vars: Vars) -> Self {
        self.inner.extend(&vars);
        self
    }

    pub fn append(&mut self, vars: &mut Vars) {
        self.inner.append(&mut vars.inner);
    }

    pub fn to_value(&self) -> JsonValue {
        JsonValue::Object(self.inner.clone())
    }
}

#[allow(unused)]
pub fn from_json(map: &JsonMap<String, JsonValue>) -> Vars {
    let mut vars = Vars::new();

    for (k, v) in map {
        let value = match v {
            JsonValue::Null => JsonValue::Null,
            JsonValue::Bool(v) => JsonValue::Bool(*v),
            JsonValue::Number(v) => from_json_number(v),
            JsonValue::String(v) => JsonValue::String(v.clone()),
            JsonValue::Array(v) => from_json_array(v),
            JsonValue::Object(v) => from_json_object(v),
        };

        vars.insert(k.to_string(), value);
    }

    vars
}

#[allow(unused)]
fn from_json_array(arr: &Vec<JsonValue>) -> JsonValue {
    let mut ret = Vec::new();
    for v in arr {
        let value = match v {
            JsonValue::Null => JsonValue::Null,
            JsonValue::Bool(v) => JsonValue::Bool(*v),
            JsonValue::Number(v) => from_json_number(v),
            JsonValue::String(v) => JsonValue::String(v.clone()),
            JsonValue::Array(v) => from_json_array(v),
            JsonValue::Object(v) => from_json_object(v),
        };
        ret.push(value);
    }

    JsonValue::Array(ret)
}

#[allow(unused)]
fn from_json_object(o: &serde_json::Map<String, JsonValue>) -> JsonValue {
    let mut map = JsonMap::new();
    for (k, v) in o {
        let value = match v {
            JsonValue::Null => JsonValue::Null,
            JsonValue::Bool(v) => JsonValue::Bool(*v),
            JsonValue::Number(v) => from_json_number(v),
            JsonValue::String(v) => JsonValue::String(v.clone()),
            JsonValue::Array(v) => from_json_array(v),
            JsonValue::Object(v) => from_json_object(v),
        };

        map.insert(k.to_string(), value);
    }

    JsonValue::Object(map)
}

#[allow(unused)]
fn from_json_number(n: &serde_json::Number) -> JsonValue {
    if n.is_i64() {
        JsonValue::Number(serde_json::Number::from(n.as_i64().unwrap()))
    } else if n.is_u64() {
        JsonValue::Number(serde_json::Number::from(n.as_u64().unwrap()))
    } else {
        JsonValue::Number(serde_json::Number::from_f64(n.as_f64().unwrap()).unwrap())
    }
}
