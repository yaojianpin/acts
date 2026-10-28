//! The participant plugin's engine hook.
//!
//! `on_init` is the one synchronous hook the engine offers during startup, so
//! it registers the package with [`Engine::add_package`](acts::Engine::add_package):
//! that writes the runtime registration directly, and the start pass publishes
//! the definition before [`EngineBuilder::start`](acts::EngineBuilder::start)
//! returns. An application that installs this plugin therefore gets
//! `acts.core.owner` without a second `add_package` call.
//!
//! P3 adds the participant resolver port to this hook; the package
//! registration stays here.

use crate::package::OwnerPackage;
use acts::{ActPlugin, Engine, Result};

/// The participant feature's engine plugin.
#[derive(Debug, Clone, Default)]
pub struct ParticipantPlugin;

impl ParticipantPlugin {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl ActPlugin for ParticipantPlugin {
    /// Register `acts.core.owner` with the engine (published by the start
    /// pass).
    ///
    /// The default participant resolution stays the engine's local one; P3
    /// installs the resolver port here.
    fn on_init(&self, engine: &Engine) -> Result<()> {
        engine.add_package::<OwnerPackage>();
        Ok(())
    }
}
