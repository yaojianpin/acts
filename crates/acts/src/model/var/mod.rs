mod variant;
mod vars;

use crate::validator::ValidatorCache;
use crate::{ActError, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::LazyLock;

pub use variant::{Variant, VariantTypes};
pub use vars::Vars;

/// Compiled `ActSchema` validators, keyed by the schema's serialized text and
/// shared by every engine in the process. Bounded (capacity, LRU, idle TTL):
/// an engine that keeps deploying workflows mints a new schema key on every
/// revision, so an unbounded map would grow with the revisions a deployment
/// has ever *seen* instead of the schemas it currently runs.
static VALIDATORS: LazyLock<ValidatorCache> = LazyLock::new(ValidatorCache::new);

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
        if self.is_empty() {
            return Ok(());
        }

        // Cache by the exact serialized schema instead of a hash. Serialization
        // is much cheaper than schema compilation, and it avoids reusing a
        // validator for a different schema on a hash collision.
        let key = serde_json::to_string(self)
            .map_err(|e| ActError::Model(format!("Schema serialization error: {e}")))?;
        let validator = VALIDATORS
            .validator(&key, || self.schema())
            .map_err(|e| ActError::Model(format!("Schema compilation error: {e}")))?;

        validator
            .validate(value)
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
