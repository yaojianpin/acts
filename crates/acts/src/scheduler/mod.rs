//! The execution runtime.
//!
//! [`Runtime`] (in `runtime.rs`) is its entry point and its outward face:
//! `Runtime::new` wires the store, the emitter, the package registry, the
//! schema cache and the shell together, and every name a consumer outside this
//! module may use is re-exported at the bottom of this file. In other words
//! `Runtime` *is* the host of the three layers below it.
//!
//! The layers are directories, in dependency order:
//!
//! - `core/` — the pure deterministic policy: the domain model (the `TaskState`
//!   vocabulary and the `view` projection model included), the planners (cron
//!   scheduling included), the commit services, recovery and the outbox. No IO,
//!   no `tokio`, no storage and no engine vocabulary (`core/architecture.rs`
//!   enforces it).
//! - `durable/` — byte-level persistence, the only layer that may name
//!   `crate::store` (and only in its declared adapter).
//! - `shell/` — the async shell: the hop driver plus its parts (`effects`,
//!   `facts`, `message`, `planning`, `predicate`, `projection`, `scope`,
//!   `action`, `errors`, `model`, `context`, `health`, `validation`, `process`,
//!   `task_tree`, `tests`). Unconstrained by design: it is the only place
//!   allowed to touch `tokio`, the engine and the store handle.
//!
//! The layers are private: nothing outside this module addresses them by path,
//! so a layer can be reshaped without a consumer noticing. Inside the runtime a
//! name is addressed where it lives (`core::plan::…`, `durable::…`,
//! `shell::…`); outside, only through the `pub use` surface below, which is the
//! module's whole contract.
//!
//! The kernel's typed error enums carry rich, caller-actionable payloads
//! (operation ids, revisions, conflicting tokens) by value — that is the
//! deliberate contract of a deterministic core.
#![allow(clippy::result_large_err)]

mod core;
mod durable;
mod runtime;
mod shell;

#[cfg(test)]
mod tests;

// The runtime's outward surface: exactly what a consumer outside this module
// needs — everything else is addressed where it lives, inside the runtime. The
// layers are private, so this list *is* the contract: a consumer that cannot
// find a name here was reaching into a layer.
//
// `Context` is the one name the crate re-exports publicly (`acts::Context`);
// the rest are crate-internal surface.

pub use self::shell::Context;

pub(crate) use self::core::model::{
    task_state::TaskState,
    view::{Node, NodeData, NodeKind, NodeTree},
};
pub(crate) use self::core::plan::cron::Cron;
pub(crate) use self::runtime::Runtime;
pub(crate) use self::shell::{Process, Task};

// `NodeContent` is the one kernel type a consumer outside the runtime needs
// only to build a row: the model's own conversion test constructs a view node
// to check what `TaskInfo` makes of it. It is test-only by nature, so it is
// re-exported as such rather than claimed as serving-path surface.
#[cfg(test)]
pub(crate) use self::core::model::view::NodeContent;

pub(crate) use self::runtime::prepare_workdir;
