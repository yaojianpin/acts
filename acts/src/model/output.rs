use crate::{ActError, Vars};
use core::fmt;
use serde::{Deserialize, Serialize, de};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Default, Clone, Serialize, Deserialize, strum::AsRefStr)]
pub enum OutputType {
    #[default]
    String,
    Bool,
    Number,
    Array,
    Object,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Output {
    pub required: bool,
    pub default: Value,
    pub r#type: OutputType,
}

#[derive(Debug, Default, Clone)]
pub struct Outputs {
    inner: HashMap<String, Output>,
}

fn get<T>(name: &str, value: &Value) -> Option<T>
where
    T: for<'de> Deserialize<'de> + Clone,
{
    if let Some(v) = value.get(name)
        && let Ok(v) = serde_json::from_value::<T>(v.clone())
    {
        Some(v)
    } else {
        None
    }
}

impl fmt::Display for OutputType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl From<Value> for Output {
    fn from(value: Value) -> Self {
        let required = get::<bool>("required", &value).unwrap_or_default();
        let default = get::<Value>("default", &value).unwrap_or_default();
        let r#type = get::<OutputType>("type", &value).unwrap_or_default();

        Self {
            required,
            default,
            r#type,
        }
    }
}

impl Serialize for Outputs {
    fn serialize<S>(&self, serializer: S) -> core::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;

        let mut map = serializer.serialize_map(Some(self.inner.len()))?;
        for (k, v) in &self.inner {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Outputs {
    fn deserialize<D>(deserializer: D) -> core::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = HashMap<String, Output>;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a map")
            }

            #[inline]
            fn visit_unit<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(HashMap::new())
            }

            #[inline]
            fn visit_map<V>(self, mut visitor: V) -> Result<Self::Value, V::Error>
            where
                V: de::MapAccess<'de>,
            {
                let mut values = HashMap::new();

                while let Some((key, value)) = visitor.next_entry::<String, Value>()? {
                    let value: Output = value.into();
                    values.insert(key, value);
                }

                Ok(values)
            }
        }

        core::result::Result::Ok(Self {
            inner: deserializer.deserialize_map(Visitor)?,
        })
    }
}

impl Outputs {
    pub fn push(&mut self, name: &str, output: &Output) {
        self.inner.insert(name.to_string(), output.clone());
    }

    pub fn check(&self, vars: &Vars) -> crate::Result<()> {
        for (k, output) in &self.inner {
            let v = vars.get::<Value>(k);
            if v.is_none() && output.required {
                return Err(ActError::Runtime(format!("the key '{k}' is required",)));
            }

            if let Some(v) = &v {
                let is_type_match = match output.r#type {
                    OutputType::String => v.is_string(),
                    OutputType::Bool => v.is_boolean(),
                    OutputType::Number => v.is_number(),
                    OutputType::Array => v.is_array(),
                    OutputType::Object => v.is_object(),
                };

                if !is_type_match {
                    return Err(ActError::Runtime(format!(
                        "the value {k}({v}) is not matched the type '{}'",
                        output.r#type
                    )));
                }
            }
        }
        Ok(())
    }
}
