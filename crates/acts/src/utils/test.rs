// Package uses constants
pub const USES_IRQ: &str = "acts.core.irq";
pub const USES_MSG: &str = "acts.core.msg";
pub const USES_SET: &str = "acts.transform.set";
pub const USES_ACTION: &str = "acts.core.action";
pub const USES_BLOCK: &str = "acts.core.block";
pub const USES_PARALLEL: &str = "acts.core.parallel";
pub const USES_SEQUENCE: &str = "acts.core.sequence";

/// The crate-wide functional-test harness. It lives with the scheduler tests
/// that first needed it, but every test module — packages, the expression
/// environment, integration cells — reaches it through this one path.
pub(crate) use crate::scheduler::tests::flow::FlowFixture;
