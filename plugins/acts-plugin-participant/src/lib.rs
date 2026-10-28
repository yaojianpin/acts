//! Participant plugin: the `acts.core.owner` IRQ package and the participant
//! extension point.
//!
//! The engine keeps the workflow model (`OwnerExpr`, `assign.owner`), the
//! durable `__assignment`/`__delegation` state and the decision
//! authorization; this crate owns the part that talks to the business system.
//!
//! Register the plugin: its `on_init` registers the `acts.core.owner` package
//! and the participant resolver with the engine. The default resolver is this
//! crate's `LocalParticipantResolver`; `ParticipantPlugin::with_resolver`
//! installs a replacement.
//!
//! ```rust,no_run
//! # use acts::Engine;
//! # async fn build() -> acts::Result<()> {
//! let engine = Engine::builder()
//!     .add_plugin(&acts_plugin_participant::ParticipantPlugin::new())
//!     .start()
//!     .await?;
//! # Ok(())
//! # }
//! ```

mod package;
mod plugin;
mod resolver;

#[cfg(test)]
mod tests;

pub use package::OwnerPackage;
pub use plugin::{PROVIDER, ParticipantPlugin};
pub use resolver::LocalParticipantResolver;
