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
// Self-contained deterministic execution core under incremental wiring (P1+
// of the kernel migration): its items are exercised by its internal test
// suite today and consumed module by module afterwards, so `dead_code` and
// `unused_imports` cannot judge reachability from the crate root yet.
#[allow(dead_code, unused_imports)]
mod kernel;
// The kernel host is the P1 shell: standalone-driven today, wired into the
// engine entry points in P2, so reachability cannot yet be judged from the
// crate root.
#[allow(dead_code)]
mod kernel_host;
// `OperationIdentity` accessors are consumed by the recovery orchestrator in
// P2; the type is constructed by `kernel::RecoveryCandidate` today.
mod export;
mod model;
mod package;
mod plugin;
#[allow(dead_code)]
mod runtime_state;
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
