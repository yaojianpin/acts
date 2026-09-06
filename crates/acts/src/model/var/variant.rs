use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Value as JsonValue, json};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum VariantTypes {
    #[default]
    String,
    Number,
    Boolean,
    Object,
    Array,
}

#[derive(Debug, Clone, Default)]
pub struct Variant {
    pub name: String,
    pub title: String,
    pub desc: String,
    pub r#type: VariantTypes,
    pub value: JsonValue,
    pub required: bool,
    /// `true` when `r#type` was explicitly declared, or derived from a
    /// literal `value`. When `false`, the schema carries no `type` constraint,
    /// so the exported value is validated/kept by its runtime JSON type
    /// instead of defaulting to `string`.
    pub typed: bool,
}

impl Variant {
    pub fn create<T>(name: &str, v: T) -> Self
    where
        T: Serialize + Clone,
    {
        let value = json!(v);
        let (r#type, typed) = match infer_variant_type(&value) {
            Some(t) => (t, true),
            // null and expression values have no static type
            None => (VariantTypes::String, false),
        };
        Self {
            name: name.to_string(),
            title: String::new(),
            desc: String::new(),
            r#type,
            value,
            required: false,
            typed,
        }
    }

    pub fn new() -> Self {
        Variant::default()
    }

    pub fn name(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    pub fn title(mut self, title: &str) -> Self {
        self.title = title.to_string();
        self
    }

    pub fn desc(mut self, desc: &str) -> Self {
        self.desc = desc.to_string();
        self
    }

    pub fn r#type(mut self, t: VariantTypes) -> Self {
        self.typed = true;
        self.r#type = t;
        self
    }

    pub fn value<T>(mut self, v: T) -> Self
    where
        T: Serialize + Clone,
    {
        let value = json!(v);
        if !self.typed
            && let Some(t) = infer_variant_type(&value)
        {
            self.r#type = t;
            self.typed = true;
        }
        self.value = value;
        self
    }

    pub fn required(mut self, v: bool) -> Self {
        self.required = v;
        self
    }
}

/// Infer the variant type from a literal JSON value. Returns `None` for
/// `null` and for `${{ ... }}` expression strings whose type is only known
/// at runtime.
fn infer_variant_type(value: &JsonValue) -> Option<VariantTypes> {
    match value {
        JsonValue::Null => None,
        JsonValue::String(text) if crate::utils::get_expr(text).is_some() => None,
        JsonValue::String(_) => Some(VariantTypes::String),
        JsonValue::Number(_) => Some(VariantTypes::Number),
        JsonValue::Bool(_) => Some(VariantTypes::Boolean),
        JsonValue::Object(_) => Some(VariantTypes::Object),
        JsonValue::Array(_) => Some(VariantTypes::Array),
    }
}

impl Serialize for Variant {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("name", &self.name)?;
        map.serialize_entry("title", &self.title)?;
        map.serialize_entry("desc", &self.desc)?;
        // an undeclared type must not be written back as an explicit one,
        // otherwise round-trips would silently re-enable strict typing
        if self.typed {
            map.serialize_entry("type", &self.r#type)?;
        }
        map.serialize_entry("value", &self.value)?;
        map.serialize_entry("required", &self.required)?;
        map.end()
    }
}

impl<'de> Deserialize<'de> for Variant {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            name: String,
            #[serde(default)]
            title: String,
            #[serde(default)]
            desc: String,
            #[serde(default)]
            r#type: Option<VariantTypes>,
            #[serde(default)]
            value: Option<JsonValue>,
            #[serde(default)]
            required: bool,
        }

        let raw = Raw::deserialize(deserializer)?;
        let (r#type, typed) = match raw.r#type {
            Some(t) => (t, true),
            None => match raw.value.as_ref().and_then(infer_variant_type) {
                Some(t) => (t, true),
                None => (VariantTypes::String, false),
            },
        };
        Ok(Variant {
            name: raw.name,
            title: raw.title,
            desc: raw.desc,
            r#type,
            value: raw.value.unwrap_or(JsonValue::Null),
            required: raw.required,
            typed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_untyped_roundtrip_keeps_untyped() {
        let var: Variant = serde_yaml::from_str("name: result\n").unwrap();
        assert_eq!(var.r#type, VariantTypes::String);
        assert!(!var.typed);
        // re-serialization must not add an explicit `type`
        let text = serde_yaml::to_string(&var).unwrap();
        assert!(!text.contains("type"));
        let var2: Variant = serde_yaml::from_str(&text).unwrap();
        assert!(!var2.typed);
    }

    #[test]
    fn variant_explicit_type_roundtrip_keeps_typed() {
        let var: Variant = serde_yaml::from_str("name: result\ntype: string\n").unwrap();
        assert_eq!(var.r#type, VariantTypes::String);
        assert!(var.typed);
        let text = serde_yaml::to_string(&var).unwrap();
        assert!(text.contains("type: string"));
        let var2: Variant = serde_yaml::from_str(&text).unwrap();
        assert!(var2.typed);
    }

    #[test]
    fn variant_literal_value_infers_type() {
        let var: Variant = serde_yaml::from_str("name: a\nvalue: 10\n").unwrap();
        assert_eq!(var.r#type, VariantTypes::Number);
        assert!(var.typed);

        let var: Variant = serde_yaml::from_str("name: b\nvalue: true\n").unwrap();
        assert_eq!(var.r#type, VariantTypes::Boolean);
        assert!(var.typed);

        let var: Variant = serde_yaml::from_str("name: c\nvalue: [1, 2]\n").unwrap();
        assert_eq!(var.r#type, VariantTypes::Array);
        assert!(var.typed);
    }

    #[test]
    fn variant_expression_value_stays_untyped() {
        let var: Variant = serde_yaml::from_str("name: result\nvalue: '${{ v }}'\n").unwrap();
        assert_eq!(var.r#type, VariantTypes::String);
        assert!(!var.typed);
    }
}
