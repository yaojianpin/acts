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
pub mod caps {
    pub use crate::scheduler::shell::caps::Capability;
    pub use crate::scheduler::shell::caps::effect::{
        EffectKey, EffectObservation, EffectOutcome, EffectRequest, EffectRunner, ResolvedEffect,
    };
    pub use crate::scheduler::shell::caps::services::CapServices;
}

/// The stable read surface of a running evaluation.
///
/// A port or package receives these values from the kernel; an embedder reads
/// them, but the kernel remains their only constructor. The surface is
/// deliberately narrower than the shell: it names the run/scope/task types a
/// port may inspect, not the internal handles they are built over.
pub mod runtime {
    pub use crate::scheduler::{Context, Scope, ScopeNode, ScopeSpec, ScopeTask, TaskState};
}

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
#[cfg(any(test, feature = "test-utils"))]
pub mod testing;
mod utils;
mod validator;

#[cfg(test)]
mod tests;

use parking_lot::RwLock;
use std::sync::Arc;

pub use crate::scheduler::{Context, HealthState, SchedulerHealth};
pub use acl::{
    ACTION_LOGIN, ACTION_LOGOUT, ACTION_REFRESH, ACTION_SETUSER, ACTION_SUBSCRIBE, ACTION_WHOAMI,
    ANONYMOUS_ALLOW, ANONYMOUS_ROLE, AccessControl, AclError, AnonymousAcl, CATALOG_GROUPS,
    DisabledAcl, LoginTokens, Principal, ScopePolicy, UserPolicy, UserSpec, action_catalog,
};
pub use builder::EngineBuilder;
pub use caps::Capability;
pub use config::{Config, ConfigLog, DEFAULT_LOG_MAX_FILES, MissingParamAction};
pub use engine::{EffectResolution, EffectUnknown, Engine};
pub use env::{ActFunction, ActUserVar};
pub use error::{ActError, Error};
pub use event::{Action, Event, Message, MessageState};
pub use export::actions;
pub use export::{
    Channel, ChannelOptions, CpuStatus, Executor, HealthStatus, LoopStatus, MemoryStatus,
    ProcessCounts, RunHandle, RunOutcome, StorageStatus, SystemStatus,
};
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
