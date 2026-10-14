//! Deciding a branch predicate on the engine's own expression environment.
//!
//! A branch `if` is an expression the legacy engine evaluated on the branch
//! task (`Branch::init` → `ctx.eval::<bool>`): the engine's environment, its
//! functions (`$ecode()`, `$cost()`, `$env`, …) and the task's variable space.
//! The kernel's built-in evaluator ([`super::evaluate_predicate`]) is a
//! self-contained subset — variables, operators and value methods — which
//! decides the common predicates without a scope, but cannot answer a
//! predicate that calls an environment function. With this seam attached the
//! host evaluates every predicate on a *headless* task of the engine, exactly
//! like a package effect runs on one, so `$ecode()` and friends mean what the
//! legacy run meant. Losing the seam (a host driving without a Runtime) falls
//! back to the subset evaluator.

use super::scope::ScopeSpec;
use crate::runtime::Runtime;
use crate::runtime::core::plan::branch::PredicateValue;
use std::sync::Arc;

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

/// Decides one branch predicate. Attach with
/// [`KernelHost::attach_predicate_runner`](super::KernelHost::attach_predicate_runner).
#[async_trait::async_trait]
pub(crate) trait PredicateRunner: Send + Sync {
    /// The predicate's value. An error is the legacy branch task's `init`
    /// error: the branch is decided `Failed`.
    async fn evaluate(&self, request: PredicateRequest) -> crate::Result<PredicateValue>;
}

/// Evaluates predicates on the engine's environment, through a headless task
/// of the declaring model.
pub(crate) struct EnginePredicateRunner {
    runtime: Arc<Runtime>,
}

impl EnginePredicateRunner {
    pub(crate) fn new(runtime: Arc<Runtime>) -> Self {
        Self { runtime }
    }
}

#[async_trait::async_trait]
impl PredicateRunner for EnginePredicateRunner {
    async fn evaluate(&self, request: PredicateRequest) -> crate::Result<PredicateValue> {
        // The overlay rides on top of the scope: both are the headless task's
        // own data, the action's vars last so they win (the legacy write
        // order).
        let mut scope = request.scope;
        for (name, value) in &request.overlay {
            scope.vars.retain(|(existing, _)| existing != name);
            scope.vars.push((name.clone(), value.clone()));
        }
        let (_, _, ctx) = super::scope::headless_scope(&self.runtime, &scope)?;
        // `ctx.eval` resolves variables, methods and the engine's functions
        // against the same environment the legacy task used; a non-boolean
        // result is a compile/eval error, which the caller turns into the
        // branch's failure.
        let value = ctx.eval::<bool>(&request.expr)?;
        Ok(if value {
            PredicateValue::True
        } else {
            PredicateValue::False
        })
    }
}
