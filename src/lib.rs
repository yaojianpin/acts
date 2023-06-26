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

use std::sync::Arc;
use std::sync::RwLock;

pub use engine::Engine;
pub use options::Options;
pub use error::ActError;
pub use event::{Action, Message};
pub use export::{Emitter, Executor, Extender, Manager};
pub use model::*;
pub use plugin::ActPlugin;
pub use rhai::Map;
pub use rhai::Module as ActModule;
pub use sch::Context;
pub use serde_json::Value as ActValue;
pub use store::{DbSet, Query, StoreAdapter};
pub type Vars = serde_json::Map<String, ActValue>;
pub type ActResult<T> = std::result::Result<T, ActError>;

pub(crate) type ShareLock<T> = Arc<RwLock<T>>;
pub(crate) use sch::{ActTask, TaskState};
