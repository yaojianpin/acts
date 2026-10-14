use crate::validator::ValidatorCache;
use crate::{ActError, ActRunAs, Result, store::Store};
use dashmap::DashMap;
use jsonschema::Validator;
use serde_json::Value as JsonValue;
use std::sync::Arc;

#[derive(Debug)]
pub(crate) struct CachedPackage {
    id: String,
    run_as: ActRunAs,
    /// The compiled-validator cache's key for this package's schema (the
    /// schema's serialized text), kept so that invalidating the package can
    /// release the validator it was the last cached holder of.
    schema_key: String,
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
    /// Package definitions, keyed by act `uses`, invalidated when the package
    /// is published, removed or re-registered. Their set is the catalogue's,
    /// so this map follows the packages the engine actually runs.
    packages: DashMap<String, Arc<CachedPackage>>,
    /// Compiled validators, deduplicated by schema content: two packages
    /// sharing a schema share one. Bounded — a catalogue that keeps
    /// republishing schemas mints a new key per revision, so an unbounded map
    /// would grow with every schema revision the engine had ever parsed.
    validators: ValidatorCache,
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
        // The cache key is the exact serialized schema rather than a hash.
        // Serialization is still cheaper than compilation, and it cannot reuse
        // a validator for a different schema because of a hash collision.
        let key = schema.to_string();
        let validator = self
            .validators
            .validator(&key, || schema.clone())
            .map_err(|err| ActError::Package(format!("schema validation error: {err}")))?;
        let cached = Arc::new(CachedPackage {
            id: package.id,
            run_as: package.run_as,
            schema_key: key,
            validator,
        });
        self.packages.insert(uses.to_string(), cached.clone());
        Ok(cached)
    }

    /// Forget the cached definition of `uses`, releasing its compiled
    /// validator when no other cached package holds the same one. The
    /// validator map dedups by schema content, so a schema another package
    /// still uses stays cached; a schema the last holder just left goes at
    /// once instead of waiting for the map's own bound.
    pub(crate) fn invalidate_package(&self, uses: &str) {
        let Some((_, package)) = self.packages.remove(uses) else {
            return;
        };
        let shared = self
            .packages
            .iter()
            .any(|other| Arc::ptr_eq(&other.validator, &package.validator));
        if !shared {
            self.validators.remove(&package.schema_key);
        }
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
        // p2 still holds the shared validator: the package going away is not
        // the schema going away.
        assert_eq!(cache.validators.len(), 1);

        let updated = data::Package {
            id: "p1".to_string(),
            schema: "{}".to_string(),
            ..Default::default()
        };
        assert!(store.packages().update(&updated).await.unwrap());
        cache.invalidate_package("p1");
        let updated = cache.package(&store, "p1").await.unwrap();
        assert!(updated.validate(&json!({})).is_ok());
        // The republished schema is its own key; the old one — still held by
        // p2 — is not the revision's to release.
        assert_eq!(cache.validators.len(), 2);
    }

    #[tokio::test]
    async fn invalidating_the_last_holder_releases_the_validator() {
        let store = Store::new(Arc::new(MemoryStore::new()));
        let package = package("p1", r#"{"type":"object","required":["x"]}"#, ActRunAs::Irq);
        assert!(store.packages().create(&package).await.unwrap());

        let cache = SchemaCache::new();
        cache.package(&store, "p1").await.unwrap();
        assert_eq!(cache.validators.len(), 1);

        cache.invalidate_package("p1");
        assert_eq!(cache.packages.len(), 0);
        assert_eq!(cache.validators.len(), 0);
        assert_eq!(cache.len(), 0);
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
