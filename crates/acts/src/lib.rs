//! A lightweight, fast, tiny, extensiable workflow engine
//!
//! [`Engine::builder`] starts an engine from a workflow model. The started
//! [`Engine`] owns its runtime, and the runtime owns the durable store and the
//! kernel that drives every run: a run is an aggregate in the store, and each
//! transition (an act's effect, a client action, a timer) is a commit against
//! that aggregate. The `procs`/`tasks`/`vars` rows are a read model projected
//! from the aggregates; `proc:get` answers from those rows, and its task read
//! re-projects a run whose rows lag its aggregate first.
//!
//! `cache_cap` bounds the runs in flight at once — runs started and not yet
//! terminal. A start beyond it is parked (its aggregate and `None` process row
//! exist, but it runs no hop) until a settled run frees a slot; the parked run
//! with the oldest `__created` stamp is admitted first. A negative value means
//! no bound.

// `include_str!` is resolved against this file when the crate is built, and the
// crate is also built from the tarball `cargo package` makes — whose root is
// `crates/acts`, where `../../../README.md` does not exist. The README the crate
// ships is the one beside its manifest.
#![doc = include_str!("../README.md")]

mod acl;
mod builder;

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

pub use crate::scheduler::Context;
pub use acl::{
    ACTION_LOGIN, ACTION_LOGOUT, ACTION_REFRESH, ACTION_SETUSER, ACTION_SUBSCRIBE, ACTION_WHOAMI,
    ANONYMOUS_ALLOW, ANONYMOUS_ROLE, AccessControl, AclError, AnonymousAcl, CATALOG_GROUPS,
    DisabledAcl, LoginTokens, Principal, ScopePolicy, UserPolicy, UserSpec, action_catalog,
};
pub use builder::EngineBuilder;
pub use config::{Config, ConfigLog, DEFAULT_LOG_MAX_FILES, MissingParamAction};
pub use engine::{EffectResolution, EffectUnknown, Engine};
pub use env::ActUserVar;
pub use error::{ActError, Error};
pub use event::{Action, Event, Message, MessageState};
pub use export::actions;
pub use export::{Channel, ChannelOptions, Executor, RunHandle, RunOutcome};
pub use model::*;
pub use package::{
    ActPackage, ActPackageCatalog, ActPackageDefinition, ActPackageRuntime, ActResource, ActRunAs,
};
pub use plugin::ActPlugin;
pub use signal::Signal;
pub use snapshot::{MAX_TTL_SECS, SnapshotEntry, SnapshotManager, SnapshotOptions, SnapshotPolicy};
pub use store::*;
pub use tokio_util::sync::CancellationToken;
pub type Result<T> = std::result::Result<T, ActError>;

pub(crate) use crate::scheduler::NodeKind;
#[cfg(test)]
pub(crate) use crate::scheduler::TaskState;
pub(crate) type ShareLock<T> = Arc<RwLock<T>>;
pub(crate) use package::Package;
