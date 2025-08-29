use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::ops::{Deref, DerefMut};

use crate::utils::consts;

#[derive(Default, Clone)]
pub struct Vars {
    inner: Map<String, Value>,
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

    pub fn with_data(self, data: serde_json::Value) -> Self {
        self.with(consts::ACT_DATA, data)
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
            Some(value)
        } else {
            None
        }
    }

    pub fn get_value(&self, name: &str) -> Option<&Value> {
        self.inner.get(name)
    }

    pub fn pop(&mut self, name: &str) -> Option<Value> {
        self.inner.remove(name)
    }

    pub fn extend(mut self, vars: Vars) -> Self {
        self.inner.extend(&vars);
        self
    }

    pub fn append(&mut self, vars: &mut Vars) {
        self.inner.append(&mut vars.inner);
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
