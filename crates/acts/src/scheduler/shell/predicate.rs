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

use crate::scheduler::{Runtime, core::plan::branch::PredicateValue};

use std::sync::Arc;

// The port lives in `capability`; this module owns the shell's default
// implementation.
pub(crate) use crate::capability::{PredicateRequest, PredicateRunner};

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
