//! A lightweight, fast, tiny, extensiable workflow engine

#![doc = include_str!("../README.md")]

mod adapter;
mod emitter;
mod engine;
mod env;
mod error;
mod executor;
mod extender;
mod manager;
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

pub use adapter::{OrgAdapter, RoleAdapter, RuleAdapter, StoreAdapter};
pub use emitter::Emitter;
pub use engine::Engine;
pub use error::ActError;
pub use executor::Executor;
pub use extender::Extender;
pub use manager::Manager;
pub use model::*;
pub use plugin::ActPlugin;
pub use rhai::Map;
pub use rhai::Module as ActModule;
pub use sch::{ActionOptions, Context, Message, UserMessage};
pub use serde_yaml::Value as ActValue;
pub type Vars = HashMap<String, ActValue>;
pub type ActResult<T> = std::result::Result<T, ActError>;

pub(crate) type ShareLock<T> = Arc<RwLock<T>>;
pub(crate) use sch::{ActTask, TaskState};
