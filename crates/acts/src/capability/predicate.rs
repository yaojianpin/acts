//! The branch-predicate port: deciding an `if` on the engine's environment.
//!
//! The kernel owns the decision; a capability implementation owns how the
//! expression is evaluated. The shell's `EnginePredicateRunner` evaluates it
//! on a headless task; a host without the port falls back to the built-in
//! subset evaluator.

use crate::scheduler::{PredicateValue, ScopeSpec};

/// One predicate to decide: the acting execution's variable space plus the
/// expression the model declared.
pub(crate) struct PredicateRequest {
    /// The variable space the headless scope sees (the acting execution's
    /// scope, the run's env and authority).
    pub(crate) scope: ScopeSpec,
    /// The branch `if` expression.
    pub(crate) expr: String,
    /// The completing action's inputs, on top of the scope: the legacy engine
    /// writes the action's vars into the task scope *before* it schedules the
    /// branches, so a predicate sees them.
    pub(crate) overlay: Vec<(String, String)>,
}

/// Decides one branch predicate. The host's serving assembly installs an
/// implementation; without one the built-in subset evaluator decides.
#[async_trait::async_trait]
pub(crate) trait PredicateRunner: Send + Sync {
    /// The predicate's value. An error is the legacy branch task's `init`
    /// error: the branch is decided `Failed`.
    async fn evaluate(&self, request: PredicateRequest) -> crate::Result<PredicateValue>;
}
