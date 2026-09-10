use crate::{ActError, ActRunAs, Result, store::Store};
use dashmap::DashMap;
use jsonschema::Validator;
use serde_json::Value as JsonValue;
use std::sync::Arc;

#[derive(Debug)]
pub(crate) struct CachedPackage {
    id: String,
    run_as: ActRunAs,
    validator: Arc<Validator>,
}

impl CachedPackage {
    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn run_as(&self) -> ActRunAs {
        self.run_as
    }

    pub(crate) fn validate(&self, instance: &JsonValue) -> Result<()> {
        self.validator.validate(instance).map_err(|err| {
            ActError::Package(format!("package({}) validation error: {}", self.id, err))
        })
    }
}

#[derive(Debug, Default)]
pub(crate) struct SchemaCache {
    packages: DashMap<String, Arc<CachedPackage>>,
    validators: DashMap<String, Arc<Validator>>,
}

impl SchemaCache {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn len(&self) -> usize {
        self.packages.len() + self.validators.len()
    }

    pub(crate) async fn package(&self, store: &Store, uses: &str) -> Result<Arc<CachedPackage>> {
        if let Some(package) = self.packages.get(uses) {
            return Ok(package.clone());
        }

        let package = store.packages().find(uses).await?;
        let schema = serde_json::from_str::<JsonValue>(&package.schema)?;
        let validator = self.validator(&schema)?;
        let cached = Arc::new(CachedPackage {
            id: package.id,
            run_as: package.run_as,
            validator,
        });
        self.packages.insert(uses.to_string(), cached.clone());
        Ok(cached)
    }

    pub(crate) fn invalidate_package(&self, uses: &str) {
        self.packages.remove(uses);
    }

    fn validator(&self, schema: &JsonValue) -> Result<Arc<Validator>> {
        // Serialize the parsed schema rather than hashing it. The compact key
        // is still cheaper than compilation and cannot reuse a validator for a
        // different schema because of a hash collision.
        let key = schema.to_string();
        if let Some(validator) = self.validators.get(&key) {
            return Ok(validator.clone());
        }

        let validator = Arc::new(
            Validator::new(schema)
                .map_err(|err| ActError::Package(format!("schema validation error: {err}")))?,
        );
        self.validators.insert(key, validator.clone());
        Ok(validator)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ActRunAs, data,
        store::{MemoryStore, Store},
    };
    use serde_json::json;

    fn package(id: &str, schema: &str, run_as: ActRunAs) -> data::Package {
        data::Package {
            id: id.to_string(),
            schema: schema.to_string(),
            run_as,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn schema_cache_reuses_validators_and_invalidates_packages() {
        let store = Store::new(Arc::new(MemoryStore::new()));
        let schema = r#"{"type":"object","required":["x"]}"#;
        let first = package("p1", schema, ActRunAs::Irq);
        let second = package("p2", schema, ActRunAs::Msg);
        assert!(store.packages().create(&first).await.unwrap());
        assert!(store.packages().create(&second).await.unwrap());

        let cache = SchemaCache::new();
        let first = cache.package(&store, "p1").await.unwrap();
        let second = cache.package(&store, "p2").await.unwrap();
        assert!(first.validate(&json!({})).is_err());
        assert!(second.validate(&json!({ "x": "ok" })).is_ok());
        assert_eq!(cache.packages.len(), 2);
        assert_eq!(cache.validators.len(), 1);

        cache.invalidate_package("p1");
        assert_eq!(cache.packages.len(), 1);

        let updated = data::Package {
            id: "p1".to_string(),
            schema: "{}".to_string(),
            ..Default::default()
        };
        assert!(store.packages().update(&updated).await.unwrap());
        cache.invalidate_package("p1");
        let updated = cache.package(&store, "p1").await.unwrap();
        assert!(updated.validate(&json!({})).is_ok());
    }

    #[tokio::test]
    async fn schema_cache_validates_func_packages() {
        let store = Store::new(Arc::new(MemoryStore::new()));
        let package = package(
            "p1",
            r#"{"type":"object","required":["x"]}"#,
            ActRunAs::Func,
        );
        assert!(store.packages().create(&package).await.unwrap());

        let cache = SchemaCache::new();
        let package = cache.package(&store, "p1").await.unwrap();
        assert!(package.validate(&json!({})).is_err());
        assert!(package.validate(&json!({ "x": "ok" })).is_ok());
    }
}
