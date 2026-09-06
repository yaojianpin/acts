mod variant;
mod vars;

use crate::{ActError, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub use variant::{Variant, VariantTypes};
pub use vars::Vars;

#[derive(Deserialize, Serialize, Debug, Default, Clone)]
#[serde(untagged)]
pub enum ActSchema {
    #[default]
    None,
    Simple(Variant),
    Multiple(Vec<Variant>),
}

impl ActSchema {
    pub fn new() -> Self {
        ActSchema::None
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, ActSchema::None)
    }

    pub fn simple(&self) -> Option<&Variant> {
        if let ActSchema::Simple(var) = self {
            Some(var)
        } else {
            None
        }
    }

    pub fn multiple(&self) -> Option<&Vec<Variant>> {
        if let ActSchema::Multiple(vars) = self {
            Some(vars)
        } else {
            None
        }
    }

    pub fn validate(&self, value: &serde_json::Value) -> Result<()> {
        let schema = self.schema();
        jsonschema::validate(&schema, value)
            .map_err(|e| ActError::Model(format!("Validation error: {e}")))
    }

    pub fn schema(&self) -> serde_json::Value {
        match self {
            ActSchema::None => serde_json::json!({}),
            ActSchema::Simple(var) => variant_property(var),
            ActSchema::Multiple(vars) => {
                let mut properties = serde_json::Map::new();
                let mut required = vec![];
                for var in vars {
                    properties.insert(var.name.clone(), variant_property(var));

                    if var.required {
                        required.push(var.name.clone());
                    }
                }
                serde_json::json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": false })
            }
        }
    }
}
/// JSON-schema property for one variant. A variant whose `type` was not
/// declared (`typed == false`) gets no `type` constraint, so any runtime
/// JSON value is accepted and keeps its own type instead of defaulting
/// to `string`.
fn variant_property(var: &Variant) -> serde_json::Value {
    let mut schema = serde_json::Map::new();
    schema.insert("name".to_string(), json!(var.name));
    schema.insert("description".to_string(), json!(var.desc));
    if var.typed {
        schema.insert("type".to_string(), json!(var.r#type));
    }
    schema.insert("defaultValue".to_string(), json!(var.value));
    serde_json::Value::Object(schema)
}
