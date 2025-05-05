pub mod core;
pub mod transform;

#[cfg(test)]
mod tests;

use crate::{Act, ActError, Engine, Result, Vars, data, scheduler::Context};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tracing::debug;

#[cfg(test)]
pub use core::RunningMode;

#[derive(Debug, Clone)]
pub struct Package {
    packages: Arc<Mutex<HashMap<String, ActPackageRegister>>>,
}

pub trait ActPackage {
    fn meta() -> ActPackageMeta;
}

pub trait ActPackageFn: Send + Sync {
    fn execute(&self, _ctx: &Context) -> Result<Option<Vars>> {
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
    #[default]
    Func,
    Irq,
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
pub struct ActPackageMeta {
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

    /// json schema for package inputs
    pub schema: serde_json::Value,

    /// package run as Irq, Msg or Func
    pub run_as: ActRunAs,

    /// package group to the package operations
    /// it is used for the editor ui to search and select the operation
    pub group: Vec<ActGroup>,

    /// package catalog
    pub catalog: ActPackageCatalog,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ActGroup {
    pub name: String,
    pub desc: String,
    pub operations: Vec<ActOperation>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ActOperation {
    pub name: String,
    pub desc: String,
}

#[derive(Debug, Clone)]
pub struct ActPackageRegister {
    pub meta: fn() -> ActPackageMeta,
    pub(crate) create: fn(&Act, &Context) -> Result<Box<dyn ActPackageFn>>,
}

impl ActPackageRegister {
    pub(crate) const fn new<T>() -> Self
    where
        T: ActPackageFn + ActPackage + DeserializeOwned + 'static,
    {
        Self {
            meta: T::meta,
            create: (|_act, ctx: &Context| {
                let meta = T::meta();

                // let inputs: serde_json::Value = ctx.task().inputs().into();
                // jsonschema::validate(&meta.schema, &inputs)
                //     .map_err(|e| ActError::ValidationError(format!("{:?}", e)))?;
                // let ret = serde_json::from_value::<T>(inputs)?;
                let params = ctx.task().params();
                jsonschema::validate(&meta.schema, &params)
                    .map_err(|e| ActError::ValidationError(format!("{:?}", e)))?;
                let ret = serde_json::from_value::<T>(params)?;
                Ok(Box::new(ret) as Box<dyn ActPackageFn>)
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
            packages: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn register(&self, name: &str, register: &ActPackageRegister) {
        let mut packages = self.packages.lock().unwrap();
        packages.insert(name.to_string(), register.clone());
    }

    pub fn get(&self, name: &str) -> Option<ActPackageRegister> {
        let registrar = self.packages.lock().unwrap();
        registrar.get(name).cloned()
    }
}

impl ActPackageMeta {
    pub fn into_data(&self) -> Result<data::Package> {
        let pack = self.clone();
        Ok(data::Package {
            id: pack.name.to_string(),
            desc: pack.desc.to_string(),
            icon: pack.icon.to_string(),
            doc: pack.doc.to_string(),
            version: pack.version.to_string(),
            schema: pack.schema.to_string(),
            run_as: pack.run_as,
            groups: serde_json::to_string(&pack.group)
                .expect("cannot convert ActPackageMeta.group to json"),
            catalog: pack.catalog,
            create_time: 0,
            update_time: 0,
            timestamp: 0,
            built_in: false,
        })
    }
}

inventory::collect!(ActPackageRegister);

pub fn init(engine: &Engine) {
    for register in inventory::iter::<ActPackageRegister> {
        let meta = (register.meta)();
        debug!("package: {}", meta.name);

        let mut pack = meta
            .into_data()
            .unwrap_or_else(|_| panic!("cannot convert ActPackageMeta to data::Package"));
        pack.built_in = true;

        engine
            .executor()
            .pack()
            .publish(&pack)
            .unwrap_or_else(|_| panic!("cannot publish package '{}'", pack.id));
        engine.runtime().package().register(meta.name, register);
    }
}
