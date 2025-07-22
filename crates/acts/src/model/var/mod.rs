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
            ActSchema::Simple(var) => {
                serde_json::json!({
                    "name": var.name,
                    "description": var.desc,
                    "type": json!(var.r#type),
                    "defaultValue": var.value,
                })
            }
            ActSchema::Multiple(vars) => {
                let mut properties = serde_json::Map::new();
                let mut required = vec![];
                for var in vars {
                    properties.insert(
                        var.name.clone(),
                        serde_json::json!({
                            "name": var.name,
                            "description": var.desc,
                            "type": json!(var.r#type),
                            "defaultValue": var.value,
                        }),
                    );

                    if var.required {
                        required.push(var.name.clone());
                    }
                }
                serde_json::json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": false })
            }
        }
    }
}
