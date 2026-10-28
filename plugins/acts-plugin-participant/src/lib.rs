//! Participant plugin: the `acts.core.owner` IRQ package and the participant
//! extension point.
//!
//! The engine keeps the workflow model (`OwnerExpr`, `assign.owner`), the
//! durable `__assignment`/`__delegation` state and the decision
//! authorization; this crate owns the part that talks to the business system.
//!
//! Register the plugin: its `on_init` registers the `acts.core.owner` package
//! with the engine, and P3 installs the participant resolver there too.
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

#[cfg(test)]
mod tests;

pub use package::OwnerPackage;
pub use plugin::ParticipantPlugin;
