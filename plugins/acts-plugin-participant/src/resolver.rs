//! The plugin's default participant resolver.
//!
//! Owner resolution is a participant feature, so the engine itself ships no
//! resolver. This adapter is what [`ParticipantPlugin::new`](crate::ParticipantPlugin::new)
//! installs: it delegates to the kernel's deterministic local evaluation
//! (`Context::resolve_local_owners`) — literal users, the context variables,
//! user-valued variables and the set operations — and leaves business-data
//! atoms to the `acts.core.owner` IRQ package.
//!
//! A custom resolver that only changes some expressions can delegate the rest
//! to this type instead of reimplementing the local evaluation. A resolver
//! that reads plugin-provided expression methods can call
//! [`Context::eval`](acts::Context::eval), which sees the engine's registered
//! `ActFunction`s.

use acts::{Context, OwnerExpr, Participant, ParticipantResolver, Result};

/// The participant plugin's default `ParticipantResolver`.
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalParticipantResolver;

impl LocalParticipantResolver {
    pub fn new() -> Self {
        Self
    }
}

impl ParticipantResolver for LocalParticipantResolver {
    fn resolve(&self, ctx: &Context, expr: &OwnerExpr) -> Result<Vec<Participant>> {
        ctx.resolve_local_owners(expr)
    }
}
