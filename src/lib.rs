//! A lightweight, fast, tiny, extensiable workflow engine

#![doc = include_str!("../README.md")]

mod adapter;
mod cache;
mod engine;
mod env;
mod error;
mod event;
mod export;
mod model;
mod options;
mod plugin;
mod sch;
mod store;
mod utils;

#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::sync::RwLock;

pub use engine::Engine;
pub use error::{ActError, Error};
pub use event::{Action, Event, Message};
pub use export::{Emitter, Executor, Extender, Manager};
pub use model::*;
pub use options::Options;
pub use plugin::ActPlugin;
pub use rhai::Map;
pub use rhai::Module as ActModule;
pub use sch::{Context, NodeKind};
pub use serde_json::Value as ActValue;
pub use store::{data, DbSet, Query, StoreAdapter};
pub type Result<T> = std::result::Result<T, ActError>;

pub(crate) type ShareLock<T> = Arc<RwLock<T>>;
pub(crate) use sch::{ActTask, TaskState};
