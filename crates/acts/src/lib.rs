//! A lightweight, fast, tiny, extensiable workflow engine

// `include_str!` is resolved against this file when the crate is built, and the
// crate is also built from the tarball `cargo package` makes — whose root is
// `crates/acts`, where `../../../README.md` does not exist. The README the crate
// ships is the one beside its manifest.
#![doc = include_str!("../README.md")]

mod acl;
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
mod snapshot;
mod store;
mod utils;
mod validator;

#[cfg(test)]
mod tests;

use parking_lot::RwLock;
use std::sync::Arc;

pub use acl::{
    ACTION_LOGIN, ACTION_LOGOUT, ACTION_REFRESH, ACTION_SETUSER, ACTION_SUBSCRIBE, ACTION_WHOAMI,
    ANONYMOUS_ALLOW, ANONYMOUS_ROLE, AccessControl, AclError, AnonymousAcl, CATALOG_GROUPS,
    DisabledAcl, LoginTokens, Principal, ScopePolicy, UserPolicy, UserSpec, action_catalog,
};
pub use builder::EngineBuilder;
pub use config::{Config, ConfigLog, DEFAULT_LOG_MAX_FILES, MissingParamAction};
pub use engine::Engine;
pub use env::ActUserVar;
pub use error::{ActError, Error};
pub use event::{Action, Event, Message, MessageState};
pub use export::actions;
pub use export::{Channel, ChannelOptions, Executor};
pub use model::*;
pub use package::{ActPackage, ActPackageCatalog, ActPackageDefinition, ActResource, ActRunAs};
pub use plugin::ActPlugin;
pub use scheduler::Context;
pub use signal::Signal;
pub use snapshot::{MAX_TTL_SECS, SnapshotEntry, SnapshotManager, SnapshotOptions, SnapshotPolicy};
pub use store::*;
pub use tokio_util::sync::CancellationToken;
pub type Result<T> = std::result::Result<T, ActError>;

pub(crate) use scheduler::NodeKind;
pub(crate) type ShareLock<T> = Arc<RwLock<T>>;
pub(crate) use package::Package;
pub(crate) use scheduler::{ActTask, TaskState};
