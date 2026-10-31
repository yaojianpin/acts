//! The capability model: one trait per extensible seam, each
//! implementation carrying its own [`Capability::key`].
//!
//! A workflow primitive and its implementation are separate: the model names
//! the primitive (`assign.owner`, a step's `uses`), the kernel defines the
//! capability trait, and the serving assembly installs one implementation per
//! trait. There is no registry and no provider selection: a slot holds one
//! implementation, attached before the engine drives operations. The
//! extensible *behavior* a workflow reaches beyond the kernel's own —
//! participant computation, a delegated join verdict — is a package the model
//! names with `uses`, not a registered provider.
//!
//! The trait and this slot structure live beside the capabilities they
//! govern: every capability is one directory under `shell/` holding its
//! contract (trait) and the kernel's own implementation.

pub(crate) mod assign;
pub(crate) mod delivery;
pub(crate) mod effect;
pub(crate) mod join;
pub(crate) mod lifecycle;
pub(crate) mod message;
pub(crate) mod predicate;
pub(crate) mod read;
pub(crate) mod services;

pub(crate) use services::CapServices;

/// One capability implementation's identity.
///
/// Every capability trait has [`Capability`] as its supertrait, so a status
/// answer or a diagnostic can name the installed implementation by its key
/// without knowing the trait.
pub trait Capability: Send + Sync {
    /// The capability's key — what `sys:status` reports and what a consumer
    /// of the port sees in errors.
    fn key(&self) -> &str;
}
