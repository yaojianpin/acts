//! A lightweight, fast, tiny, extensiable workflow engine

#![doc = include_str!("../README.md")]

mod adapter;
mod builder;
mod cache;
mod config;
mod engine;
mod env;
mod error;
mod event;
mod export;
mod model;
mod plugin;
mod scheduler;
mod signal;
mod store;
mod utils;

#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::sync::RwLock;

pub use builder::Builder;
pub use config::Config;
pub use engine::Engine;
pub use env::ActModule;
pub use error::{ActError, Error};
pub use event::{Action, Event, Message, MessageState};
pub use export::{Channel, ChannelOptions, Executor, ExecutorQuery, Extender};
pub use model::*;
pub use plugin::ActPlugin;
pub use signal::Signal;
pub use store::{data, DbSet, PageData, Query, StoreAdapter};
pub type Result<T> = std::result::Result<T, ActError>;

pub(crate) use scheduler::{Context, NodeKind};
pub(crate) type ShareLock<T> = Arc<RwLock<T>>;
pub(crate) use scheduler::{ActTask, TaskState};
