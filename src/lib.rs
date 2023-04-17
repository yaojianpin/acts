//! A lightweight, fast, tiny, extensiable workflow engine

#![doc = include_str!("../README.md")]

mod adapter;
mod engine;
mod env;
mod error;
pub mod event;
mod export;
mod model;
mod options;
mod plugin;
mod sch;
pub mod store;
mod utils;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;

pub use adapter::{OrgAdapter, RoleAdapter, RuleAdapter};
pub use engine::Engine;
pub use error::ActError;
pub use event::{ActionOptions, Message, UserMessage};
pub use export::{Emitter, Executor, Extender, Manager};
pub use model::*;
pub use plugin::ActPlugin;
pub use rhai::Map;
pub use rhai::Module as ActModule;
pub use sch::Context;
pub use serde_yaml::Value as ActValue;
pub use store::{DbSet, Query, StoreAdapter};
pub type Vars = HashMap<String, ActValue>;
pub type ActResult<T> = std::result::Result<T, ActError>;

pub(crate) type ShareLock<T> = Arc<RwLock<T>>;
pub(crate) use sch::{ActTask, TaskState};
