//! A lightweight, fast, tiny, extensiable workflow engine

#![doc = include_str!("../../../README.md")]

mod builder;
mod cache;
mod config;
mod engine;
mod env;
mod error;
mod event;
mod export;
mod model;
mod package;
mod plugin;
mod scheduler;
mod signal;
mod store;
mod utils;

#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::sync::RwLock;

pub use builder::EngineBuilder;
pub use config::Config;
pub use engine::Engine;
pub use env::ActUserVar;
pub use error::{ActError, Error};
pub use event::{Action, Event, Message, MessageState};
pub use export::{Channel, ChannelOptions, Executor, Extender};
pub use model::*;
pub use package::{
    ActOperation, ActPackage, ActPackageCatalog, ActPackageMeta, ActResource, ActRunAs,
};
pub use plugin::ActPlugin;
pub use scheduler::Context;
pub use signal::Signal;
pub use store::{DbCollection, PageData, data, query};
pub type Result<T> = std::result::Result<T, ActError>;

pub(crate) use scheduler::NodeKind;
pub(crate) type ShareLock<T> = Arc<RwLock<T>>;
pub(crate) use package::Package;
pub(crate) use scheduler::{ActTask, TaskState};
