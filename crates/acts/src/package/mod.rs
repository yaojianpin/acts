pub mod core;
pub mod transform;

#[cfg(test)]
mod tests;

use crate::{
    ActError, Config, Engine, Result, Vars, data,
    scheduler::{Context, Runtime},
    store::DbCollectionIden,
};
use dashmap::DashMap;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{fmt, sync::Arc};
use tracing::debug;

#[cfg(test)]
pub use core::RunningMode;

struct PackageEntry {
    register: ActPackageRegister,
    instance: Mutex<Option<Arc<dyn ActPackage>>>,
}

type SharedPackageEntry = Arc<PackageEntry>;

impl fmt::Debug for PackageEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PackageEntry")
            .field("register", &self.register)
            .finish()
    }
}

#[derive(Clone)]
pub struct Package {
    packages: Arc<DashMap<String, SharedPackageEntry>>,
}

impl fmt::Debug for Package {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Package")
            .field("packages", &self.packages)
            .finish()
    }
}

#[async_trait::async_trait]

pub trait ActPackage: Send + Sync {
    /// create package instance with config
    fn new(config: &Config) -> Result<Self>
    where
        Self: Sized;
    /// get package meta definition
    fn definition() -> ActPackageDefinition
    where
        Self: Sized;
    /// executing with task context
    async fn execute(&self, _ctx: &Context, _params: &serde_json::Value) -> Result<Option<Vars>> {
        Ok(None)
    }
    /// start with non-context, such as workflow event
    async fn start(
        &self,
        _rt: &Arc<Runtime>,
        _params: &serde_json::Value,
        _options: &Vars,
    ) -> Result<Option<Vars>> {
        Ok(None)
    }
}

#[derive(
    Serialize,
    Deserialize,
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    strum::AsRefStr,
    strum::EnumString,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum ActRunAs {
    /// only used internally
    Func,
    /// interrupt request, need to response
    #[default]
    Irq,
    /// message without response
    Msg,
}

#[derive(
    Serialize,
    Deserialize,
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    strum::AsRefStr,
    strum::EnumString,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum ActPackageCatalog {
    /// acts core packages
    Core,

    /// workflow event
    Event,

    /// workflow trace package
    Output,

    /// data transform
    Transform,

    /// form submition
    Form,

    /// AI related for LLMs
    Ai,

    /// the other applications to integrate into acts
    /// such as Store, State, Observability, Pubsub
    #[default]
    App,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ActPackageDefinition {
    /// package id, used to identify the package
    pub id: &'static str,

    /// package simple name
    pub name: &'static str,

    /// package description
    pub desc: &'static str,

    /// icon name to display in the editor ui
    pub icon: &'static str,

    /// releated doc url to show the help
    pub doc: &'static str,

    /// package version
    pub version: &'static str,

    /// json schema for package params
    pub schema: serde_json::Value,

    /// extra options
    #[serde(default)]
    pub options: Option<serde_json::Value>,

    /// package run as Irq, Msg or Func
    /// Func is only used internally
    pub run_as: ActRunAs,

    /// package resources to the orgnize multiple resources
    /// it is used for the editor ui to search and select the resources
    /// each resource value can fill the special value into the UI
    pub resources: Vec<ActResource>,

    /// package catalog
    pub catalog: ActPackageCatalog,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ActResource {
    pub name: String,
    pub desc: String,
    pub value: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ActPackageRegister {
    pub meta: fn() -> ActPackageDefinition,
    pub create: fn(config: &Config) -> Result<Arc<dyn ActPackage>>,
}

impl ActPackageRegister {
    pub(crate) const fn new<T>() -> Self
    where
        T: ActPackage + 'static,
    {
        Self {
            meta: T::definition,
            create: (|config: &Config| {
                // let meta = T::definition();
                // jsonschema::validate(&meta.schema, params).map_err(|err| {
                //     ActError::Package(format!(
                //         "package({}) schema validation error: {}",
                //         meta.id, err
                //     ))
                // })?;

                let ret = T::new(config)?;
                Ok(Arc::new(ret) as Arc<dyn ActPackage>)
            }),
        }
    }
}

impl Default for Package {
    fn default() -> Self {
        Self::new()
    }
}

impl Package {
    pub fn new() -> Self {
        Self {
            packages: Arc::new(DashMap::new()),
        }
    }

    pub fn register(&self, id: &str, register: &ActPackageRegister) {
        // Replacing the entry also replaces its instance slot, so an old
        // registration can never initialize a cache entry for a new one.
        self.packages.insert(
            id.to_string(),
            Arc::new(PackageEntry {
                register: register.clone(),
                instance: Mutex::new(None),
            }),
        );
    }

    pub fn get(&self, id: &str) -> Option<ActPackageRegister> {
        self.packages.get(id).map(|entry| entry.register.clone())
    }

    /// Return the cached instance for a registration, creating it on first use.
    ///
    /// Package constructors are intended to initialize reusable resources (or
    /// leave them to async execution), so this avoids rebuilding a package for
    /// every act/event and lets async packages keep one connection alive.
    pub(crate) fn create(&self, id: &str, config: &Config) -> Result<Arc<dyn ActPackage>> {
        let entry = self
            .packages
            .get(id)
            .ok_or_else(|| ActError::Runtime(format!("cannot find package '{id}'")))?
            .clone();
        let mut instance = entry.instance.lock();

        if let Some(package) = &*instance {
            return Ok(package.clone());
        }

        let package = (entry.register.create)(config)?;
        *instance = Some(package.clone());
        Ok(package)
    }
}

impl ActPackageDefinition {
    pub fn into_data(&self) -> Result<data::Package> {
        let pack = self.clone();
        Ok(data::Package {
            id: pack.id.to_string(),
            name: pack.name.to_string(),
            desc: pack.desc.to_string(),
            icon: pack.icon.to_string(),
            doc: pack.doc.to_string(),
            version: pack.version.to_string(),
            schema: pack.schema.to_string(),
            options: pack.options.map(|v| v.to_string()),
            run_as: pack.run_as,
            resources: serde_json::to_string(&pack.resources)
                .expect("cannot convert ActPackageMeta.resources to json"),
            catalog: pack.catalog,
            create_time: 0,
            update_time: 0,
            timestamp: 0,
            built_in: false,
            v: data::Package::version(),
        })
    }
}

inventory::collect!(ActPackageRegister);

pub async fn init(engine: &Engine) -> Result<()> {
    for register in inventory::iter::<ActPackageRegister> {
        let meta = (register.meta)();
        debug!("package: {}", meta.name);

        let mut pack = meta.into_data()?;
        pack.built_in = true;
        engine.executor().pack().publish(&pack).await?;
        engine.runtime().package().register(meta.id, register);
    }
    Ok(())
}

#[cfg(test)]
mod cached_instance_tests {
    use super::*;
    use serde_json::json;
    #[derive(Clone, Debug)]
    struct CachedPackage;

    #[async_trait::async_trait]
    impl ActPackage for CachedPackage {
        fn new(_config: &Config) -> Result<Self> {
            Ok(Self)
        }

        fn definition() -> ActPackageDefinition {
            ActPackageDefinition {
                id: "test.cached",
                name: "Cached",
                desc: "",
                icon: "",
                doc: "",
                version: "0.1.0",
                schema: json!({}),
                options: None,
                run_as: ActRunAs::Func,
                resources: vec![],
                catalog: ActPackageCatalog::Core,
            }
        }
    }

    #[test]
    fn create_reuses_instance_until_registration_is_replaced() {
        let package = Package::new();
        package.register(
            CachedPackage::definition().id,
            &ActPackageRegister::new::<CachedPackage>(),
        );
        let config = Config::default();

        let first = package.create("test.cached", &config).unwrap();
        let second = package.create("test.cached", &config).unwrap();
        assert!(Arc::ptr_eq(&first, &second));

        package.register(
            CachedPackage::definition().id,
            &ActPackageRegister::new::<CachedPackage>(),
        );
        let third = package.create("test.cached", &config).unwrap();
        assert!(!Arc::ptr_eq(&first, &third));
    }
}
