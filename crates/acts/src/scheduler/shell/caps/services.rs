//! The typed port structure the kernel host is assembled from.

use super::Capability;
use crate::scheduler::shell::caps::{
    assign::AssignmentResolver, delivery::OutboxTransport, effect::EffectRunner, join::Join,
    lifecycle::LifecycleSink, predicate::PredicateRunner,
};
use std::sync::Arc;

/// The kernel's typed ports.
///
/// This is the structure the scheduler's host is assembled from: the scheduler
/// calls a capability, never a concrete shell implementation. Each field is
/// one capability slot holding exactly one implementation — the serving
/// assembly attaches the defaults after construction, and a test may replace
/// one the same way. There is no registry: a slot is a single `Arc`.
///
/// - `effects`, `predicates`, `delivery` and `lifecycle` are the shell's
///   extensible seams. Their request/response types are still the runtime's
///   internal vocabulary, so most fields are crate-visible until the runtime
///   API is public.
///
/// Projection, storage and the store-facing timers stay in the shell: they are
/// kernel facilities, not extension points.
pub struct CapServices {
    /// Package effects. `None` fails an act that needs one — a host must
    /// attach its runner before driving operations.
    pub effects: Option<Arc<dyn EffectRunner>>,
    /// Branch predicates. `None` falls back to the built-in subset evaluator.
    pub(crate) predicates: Option<Arc<dyn PredicateRunner>>,
    /// Durable outbox delivery. The shadow transport (records logged, nothing
    /// sent) is the default a serving assembly replaces.
    pub(crate) delivery: Arc<dyn OutboxTransport>,
    /// Process lifecycle reports. `None` reports nothing.
    pub(crate) lifecycle: Option<Arc<dyn LifecycleSink>>,
    /// The container join capability: the counted rules' census decision and
    /// the delegated verdict mapping (the kernel's own implementation).
    pub(crate) join: Join,
    /// The owner-resolution seam: how an act's declared `assign.owner`
    /// expression becomes its one owner at start. `None` fails a declared
    /// assignment closed — an act never runs unowned.
    pub(crate) assignment: Option<Arc<dyn AssignmentResolver>>,
    /// The read model's write-behind projector: materializes the legacy row
    /// collections from every committed snapshot. `None` runs without a read
    /// model (the kernel's own tests).
    pub(crate) read: Option<Arc<super::read::projection::RowProjector>>,
}

impl CapServices {
    /// The installed capabilities' keys, in slot order: what `sys:status`
    /// reports as the engine's `caps` list.
    pub(crate) fn keys(&self) -> Vec<String> {
        let mut keys = Vec::new();
        if let Some(effects) = &self.effects {
            keys.push(effects.key().to_string());
        }
        if let Some(predicates) = &self.predicates {
            keys.push(predicates.key().to_string());
        }
        keys.push(self.delivery.key().to_string());
        if let Some(lifecycle) = &self.lifecycle {
            keys.push(lifecycle.key().to_string());
        }
        keys.push(self.join.key().to_string());
        if let Some(assignment) = &self.assignment {
            keys.push(assignment.key().to_string());
        }
        if let Some(read) = &self.read {
            keys.push(read.key().to_string());
        }
        keys
    }
}
