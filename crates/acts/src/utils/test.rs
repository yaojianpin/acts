// Package uses constants
pub const USES_IRQ: &str = "acts.core.irq";
pub const USES_MSG: &str = "acts.core.msg";
pub const USES_SET: &str = "acts.transform.set";

/// The crate-wide functional-test harness. It lives with the scheduler tests
/// that first needed it, but every test module — packages, the expression
/// environment, integration cells — reaches it through this one path.
pub(crate) use crate::scheduler::tests::flow::FlowFixture;
