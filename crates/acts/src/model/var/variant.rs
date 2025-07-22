use serde::{Deserialize, Serialize};
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Variant {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub desc: String,
    #[serde(default)]
    pub r#type: VariantTypes,
    #[serde(default)]
    pub value: JsonValue,
    #[serde(default)]
    pub required: bool,
}

impl Variant {
    pub fn create<T>(name: &str, v: T) -> Self
    where
        T: Serialize + Clone,
    {
        let value = json!(v);
        let r#type = match &value {
            JsonValue::String(_) => VariantTypes::String,
            JsonValue::Number(_) => VariantTypes::Number,
            JsonValue::Bool(_) => VariantTypes::Boolean,
            JsonValue::Object(_) => VariantTypes::Object,
            JsonValue::Array(_) => VariantTypes::Array,
            _ => VariantTypes::String,
        };
        Self {
            name: name.to_string(),
            title: String::new(),
            desc: String::new(),
            r#type,
            value,
            required: false,
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
        self.r#type = t;
        self
    }

    pub fn value<T>(mut self, v: T) -> Self
    where
        T: Serialize + Clone,
    {
        self.value = json!(v);
        self
    }

    pub fn required(mut self, v: bool) -> Self {
        self.required = v;
        self
    }
}
