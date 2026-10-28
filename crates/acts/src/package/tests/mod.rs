//! Package behavior cells: one module per built-in package, exercising the
//! logic the package contributes to a run (not only its schema).
//!
//! The pre-kernel suite lived in `crates/acts/src/package/tests`; these cells
//! keep its intent and use the shared [`FlowFixture`] from
//! `scheduler::tests::flow`, so a package test is an ordinary workflow run.
//! Every test name is prefixed with the package's module abbreviation
//! (`pack_irq_`, `pack_msg_`, …).

use crate::{Context, Engine, Vars, Workflow, model::Step};

pub(crate) use crate::utils::test::FlowFixture;

pub(crate) mod action;
pub(crate) mod block;
pub(crate) mod irq;
pub(crate) mod msg;
pub(crate) mod parallel;
pub(crate) mod sequence;
pub(crate) mod set;
pub(crate) mod subflow;

pub(crate) const USES_ACTION: &str = "acts.core.action";
pub(crate) const USES_BLOCK: &str = "acts.core.block";
pub(crate) const USES_PARALLEL: &str = "acts.core.parallel";
pub(crate) const USES_SEQUENCE: &str = "acts.core.sequence";
pub(crate) const USES_SUBFLOW: &str = "acts.core.subflow";

pub(crate) use crate::utils::test::{USES_IRQ, USES_MSG, USES_SET};

/// The root context of a finished run — the carrier `ActPackage::execute`
/// reads. Tests that call `execute` directly need one, and the params they
/// pass cannot come from a run: the package schema would refuse them before
/// the package's own parsing runs.
pub(crate) async fn completed_context() -> Context {
    let engine = Engine::builder().start().await.unwrap();
    let sig = engine.signal(());
    let done = sig.clone();
    engine.channel().on_complete(move |_| {
        let done = done.clone();
        async move { done.close() }
    });
    let workflow = Workflow::new()
        .with_id("package-ctx")
        .with_step(|step| step.with_id("s1"));
    let pid = engine
        .runtime()
        .start(&workflow, Vars::new())
        .await
        .unwrap();
    sig.recv().await;
    let runtime = engine.runtime();
    let read = runtime.read_proc(&pid).await.unwrap().unwrap();
    crate::scheduler::read_context(&runtime, &read).unwrap()
}

/// A one-step workflow whose only step runs `uses` with `params`.
pub(crate) fn one_step(id: &str, uses: &str, params: Vars) -> Workflow {
    let mut workflow = Workflow::new().with_id(id);
    workflow.steps.push(Step {
        id: "s1".to_string(),
        name: "s1".to_string(),
        uses: Some(uses.to_string()),
        params: params.to_value(),
        ..Default::default()
    });
    workflow
}
