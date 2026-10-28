//! The typed port structure the kernel host is assembled from.

use super::{
    EffectRunner, LifecycleSink, OutboxTransport, ParticipantResolver, PortSet, PredicateRunner,
};

/// The kernel's typed ports.
///
/// This is the structure the scheduler's host is assembled from: the scheduler
/// calls a capability, never a concrete shell implementation. Each field is one
/// capability type, and the implementations inside it are keyed by the
/// provider key a plugin or the runtime's default assembly chose.
///
/// Only extensible capabilities live here:
/// - `participants` is the public participant capability;
/// - `effects`, `predicates`, `delivery` and `lifecycle` are the shell's
///   extensible seams. Their request/response types are still the runtime's
///   internal vocabulary, so the fields are crate-visible until the runtime
///   API is public.
///
/// Projection, storage and the store-facing timers stay in the shell: they are
/// kernel facilities, not extension points. The shell's async assignment
/// adapter also stays with the kernel; it is the bridge from a headless scope
/// to the participant capability, not a second provider-facing port.
#[derive(Default)]
pub struct KernelPorts {
    /// The provider-facing participant resolver capability.
    pub participants: PortSet<dyn ParticipantResolver>,
    /// Package effects.
    pub(crate) effects: PortSet<dyn EffectRunner>,
    /// Branch predicates.
    pub(crate) predicates: PortSet<dyn PredicateRunner>,
    /// Durable outbox delivery.
    pub(crate) delivery: PortSet<dyn OutboxTransport>,
    /// Process lifecycle reports.
    pub(crate) lifecycle: PortSet<LifecycleSink>,
}
