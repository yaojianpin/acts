pub mod core;
pub mod event;
pub mod transform;

#[cfg(test)]
mod tests;

use crate::{
    Config, Engine, Result, Vars, data,
    scheduler::{Context, Runtime},
    store::DbCollectionIden,
};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::{fmt::Debug, sync::Arc};
use tracing::debug;

#[cfg(test)]
pub use core::RunningMode;

#[derive(Debug, Clone)]
pub struct Package {
    packages: Arc<DashMap<String, ActPackageRegister>>,
}

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
    fn execute(&self, _ctx: &Context, _params: &serde_json::Value) -> Result<Option<Vars>> {
        Ok(None)
    }
    /// start with non-context, such as workflow event
    fn start(
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
        self.packages.insert(id.to_string(), register.clone());
    }

    pub fn get(&self, id: &str) -> Option<ActPackageRegister> {
        self.packages.get(id).map(|v| v.clone())
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

pub fn init(engine: &Engine) -> Result<()> {
    for register in inventory::iter::<ActPackageRegister> {
        let meta = (register.meta)();
        debug!("package: {}", meta.name);

        let mut pack = meta.into_data()?;
        pack.built_in = true;
        engine.executor().pack().publish(&pack)?;
        engine.runtime().package().register(meta.id, register);
    }
    Ok(())
}
