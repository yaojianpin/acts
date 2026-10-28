//! The participant plugin's engine hook.
//!
//! `on_init` is the one synchronous hook the engine offers during startup, so
//! it registers the package with [`Engine::add_package`](acts::Engine::add_package):
//! that writes the runtime registration directly, and the start pass publishes
//! the definition before [`EngineBuilder::start`](acts::EngineBuilder::start)
//! returns. An application that installs this plugin therefore gets
//! `acts.core.owner` without a second `add_package` call.
//!
//! The same hook installs the participant resolver port: the default
//! [`LocalParticipantResolver`](crate::LocalParticipantResolver), or the
//! replacement passed to [`ParticipantPlugin::with_resolver`]. The package
//! registration stays here.

use crate::{package::OwnerPackage, resolver::LocalParticipantResolver};
use acts::{
    ActPlugin, Engine, ParticipantResolver, Result,
    capability::{CapabilityKind, CapabilityProvider},
};
use std::sync::Arc;

/// The provider key this plugin registers for the participant capability.
///
/// The kernel does not define this string: it is the plugin's external
/// identity, used by the capability table and the resolver port.
pub const PROVIDER: &str = "acts-plugin-participant";

/// The participant feature's engine plugin.
#[derive(Clone)]
pub struct ParticipantPlugin {
    resolver: Arc<dyn ParticipantResolver>,
}

impl std::fmt::Debug for ParticipantPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParticipantPlugin")
            .field("resolver", &"<dyn ParticipantResolver>")
            .finish()
    }
}

impl Default for ParticipantPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ParticipantPlugin {
    /// The plugin with its local participant resolver.
    pub fn new() -> Self {
        Self::with_resolver(Arc::new(LocalParticipantResolver))
    }

    /// The plugin with a custom resolver for `assign.owner`'s local atoms.
    ///
    /// The resolver must be synchronous and I/O-free. A resolver that only
    /// changes some expressions can delegate the rest to
    /// [`LocalParticipantResolver`] rather than reimplementing the local
    /// evaluation.
    pub fn with_resolver(resolver: Arc<dyn ParticipantResolver>) -> Self {
        Self { resolver }
    }
}

#[async_trait::async_trait]
impl ActPlugin for ParticipantPlugin {
    /// Register the participant capability, `acts.core.owner` and the
    /// participant resolver with the engine (the start pass publishes the
    /// package definition and seals the capability and resolver slots).
    fn on_init(&self, engine: &Engine) -> Result<()> {
        engine.register_capability_provider(CapabilityProvider {
            kind: CapabilityKind::Participant,
            key: PROVIDER.to_string(),
            plugin: PROVIDER.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        })?;
        engine.add_package::<OwnerPackage>();
        engine.register_participant_resolver(PROVIDER, self.resolver.clone())?;
        Ok(())
    }
}
