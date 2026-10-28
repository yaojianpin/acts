//! The participant resolver port.
//!
//! `assign.owner` resolves its expression through the resolver installed on the
//! engine. Owner resolution is a participant feature, so the engine ships no
//! built-in resolver: `acts-plugin-participant` installs its
//! `LocalParticipantResolver` (the deterministic evaluation of the local atoms
//! — a literal user, the context variables, user-valued variables and the set
//! operations) from `ActPlugin::on_init`. Without a resolver a declared
//! assignment fails closed, naming the plugin.
//!
//! A plugin or embedder may install a replacement from `ActPlugin::on_init` to
//! change *who* a local expression names (a directory, a naming rule, an audit
//! hook), but not *who may decide*: `__assignment`/`__delegation` remain the
//! kernel's only authorization source. A replacement that only changes some
//! expressions can delegate the rest to
//! [`Context::resolve_local_owners`](crate::Context::resolve_local_owners),
//! which is the same local evaluation the default resolver uses.
//!
//! The port is synchronous and I/O-free by contract: assignment runs on an
//! act's start path. Business data (roles, groups, resource owners, relations)
//! still enters a workflow only through the `acts.core.owner` IRQ package; a
//! resolver that resolves those atoms itself would move a business directory
//! into the engine's start path, which this port deliberately does not allow.

use crate::{
    Result,
    model::{OwnerExpr, Participant},
    scheduler::Context,
};

/// Resolve the participants an [`OwnerExpr`] names.
///
/// The engine calls this from `assign.owner` and from the owner package's
/// parameter preparation. It must be synchronous, do no network or store I/O,
/// and return the participants in a deterministic order. Business-data atoms
/// are the default implementation's errors, not something a custom resolver is
/// expected to resolve here: send the expression to the business system with
/// the `acts.core.owner` package instead.
///
/// An expression an implementation cannot resolve should delegate the
/// non-local cases to
/// [`Context::resolve_local_owners`](crate::Context::resolve_local_owners)
/// rather than duplicating the local evaluation (the participant plugin's
/// `LocalParticipantResolver` is this adapter).
pub trait ParticipantResolver: Send + Sync {
    fn resolve(&self, ctx: &Context, expr: &OwnerExpr) -> Result<Vec<Participant>>;
}
