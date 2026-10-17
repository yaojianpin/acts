//! The pure deterministic core: domain model, planners, commit services, recovery and outbox policy.
//!
//! Deliberately free of IO, `tokio`, storage and engine vocabulary — see `architecture.rs`.

#[cfg(test)]
pub(crate) mod architecture;
pub(crate) mod commit;
#[cfg(test)]
pub(crate) mod coverage;
pub(crate) mod model;
pub(crate) mod outbox;
pub(crate) mod plan;
pub(crate) mod ready;
pub(crate) mod recovery;
