//! Package behavior cells: one module per built-in package, exercising the
//! logic the package contributes to a run (not only its schema).
//!
//! The pre-kernel suite lived in `crates/acts/src/package/tests`; these cells
//! keep its intent and use the shared [`FlowFixture`] from
//! `scheduler::tests::flow`, so a package test is an ordinary workflow run.
//! Every test name is prefixed with the package's module abbreviation
//! (`pack_irq_`, `pack_msg_`, …).

use crate::{Vars, Workflow, model::Step};

pub(crate) use crate::scheduler::tests::flow::FlowFixture;

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
