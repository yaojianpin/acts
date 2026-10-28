//! Typed kernel capabilities.
//!
//! A workflow primitive and its implementation are separate: the model names
//! the capability type it needs, a plugin advertises a provider key, and
//! [`KernelPorts`] maps that key to the typed implementation. The model
//! attribute (`assign.owner`) is the stable contract; a plugin adds a feature
//! by advertising a capability for a kernel trait rather than by growing the
//! shell.
//!
//! Only the extensible seams live here: participant resolution, package
//! effects, branch predicates, outbox delivery and process lifecycle. The
//! kernel's storage, projection and store-facing timer loops stay in the shell
//! because they are not extension points. Provider selection and publish/start
//! checks remain on the engine's current transitional path.

mod delivery;
mod effect;
mod kernel_ports;
mod kind;
mod lifecycle;
mod participant;
mod port_set;
mod predicate;

pub use kernel_ports::KernelPorts;
pub use kind::{CapabilityKind, CapabilityProvider, CapabilityRequirement};
pub use participant::ParticipantResolver;
pub use port_set::PortSet;

pub(crate) use delivery::{DeliveryOutcome, OutboxTransport};
pub(crate) use effect::{
    EffectObservation, EffectOutcome, EffectRequest, EffectRunner, ResolvedEffect,
};
pub(crate) use lifecycle::{LifecycleSink, ProcessEvent};
pub(crate) use predicate::{PredicateRequest, PredicateRunner};
