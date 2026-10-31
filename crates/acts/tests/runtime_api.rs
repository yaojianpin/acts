//! The runtime read surface is nameable from an external crate.
//!
//! This is the P3-facing contract: a port or embedder can name the scope and
//! task types the kernel hands it, even though only the kernel constructs
//! them. A type that accidentally becomes `pub(crate)` breaks this test at
//! compile time.

use acts::runtime::{Context, Scope, ScopeNode, ScopeSpec, ScopeTask, TaskState};

#[test]
fn runtime_types_are_publicly_nameable() {
    fn context(_: Option<&Context>) {}
    fn scope(
        _: Option<&Scope>,
        _: Option<&ScopeTask>,
        _: Option<&ScopeNode>,
        _: Option<&ScopeSpec>,
        _: Option<TaskState>,
    ) {
    }

    context(None);
    scope(None, None, None, None, None);

    // The read accessors are part of the surface, not just the type names.
    let _ = ScopeSpec::process_id;
    let _ = ScopeSpec::node;
    let _ = ScopeSpec::model;
    let _ = ScopeSpec::pushed;
    let _ = ScopeSpec::vars;
    let _ = ScopeSpec::env;
    let _ = ScopeSpec::owner;
    let _ = ScopeTask::state;
    let _ = Context::task;
}

use acts::caps::{
    Capability, EffectKey, EffectOutcome, EffectRequest, EffectRunner, ResolvedEffect,
};

/// An external effect runner: only the public request/response vocabulary is
/// used. The trait stays implementable outside the crate; installation is the
/// serving assembly's job, not a registration API.
#[derive(Clone)]
struct DummyEffect;

impl acts::caps::Capability for DummyEffect {
    fn key(&self) -> &str {
        "test.effect"
    }
}

#[async_trait::async_trait]
impl EffectRunner for DummyEffect {
    async fn resolve(&self, _request: &EffectRequest) -> acts::Result<ResolvedEffect> {
        Ok(ResolvedEffect {
            params: serde_json::json!({}),
            options: serde_json::json!({}),
        })
    }

    async fn run(&self, _request: EffectRequest, _key: EffectKey) -> acts::Result<EffectOutcome> {
        Ok(EffectOutcome::empty())
    }
}

#[test]
fn effect_runner_is_implementable_with_a_key() {
    fn assert_capability(_: &dyn Capability) {}
    assert_capability(&DummyEffect);
    assert_eq!(DummyEffect.key(), "test.effect");
}
