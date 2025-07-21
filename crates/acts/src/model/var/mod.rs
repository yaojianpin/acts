mod variant;
mod vars;

use crate::{ActError, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub use variant::{Variant, VariantTypes};
pub use vars::Vars;

#[derive(Deserialize, Serialize, Debug, Default, Clone)]
#[serde(untagged)]
pub enum ActValue {
    #[default]
    None,
    Var(Variant),
    Vars(Vec<Variant>),
}

impl PartialEq for ActValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ActValue::None, ActValue::None) => true,
            (ActValue::Var(v1), ActValue::Var(v2)) => v1.name == v2.name && v1.value == v2.value,
            (ActValue::Vars(v1), ActValue::Vars(v2)) => {
                if v1.len() != v2.len() {
                    return false;
                }
                for v in v1 {
                    if !v2.iter().any(|v2| v2.name == v.name && v2.value == v.value) {
                        return false;
                    }
                }
                true
            }
            _ => false,
        }
    }
}

impl ActValue {
    pub fn new() -> Self {
        ActValue::None
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, ActValue::None)
    }

    pub fn validate(&self, value: &serde_json::Value) -> Result<()> {
        let schema = self.to_schema();
        jsonschema::validate(&schema, value)
            .map_err(|e| ActError::Model(format!("Validation error: {e}")))
    }

    fn to_schema(&self) -> serde_json::Value {
        match self {
            ActValue::None => serde_json::json!({}),
            ActValue::Var(var) => {
                serde_json::json!({
                    "name": var.name,
                    "description": var.desc,
                    "type": json!(var.r#type),
                    "defaultValue": var.value
                })
            }
            ActValue::Vars(vars) => {
                let mut properties = serde_json::Map::new();
                for var in vars {
                    properties.insert(
                        var.name.clone(),
                        serde_json::json!({
                            "name": var.name,
                            "description": var.desc,
                            "type": json!(var.r#type),
                            "defaultValue": var.value
                        }),
                    );
                }
                serde_json::json!({ "type": "object", "properties": properties })
            }
        }
    }
}
