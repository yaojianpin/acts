// Package uses constants
pub const USES_IRQ: &str = "acts.core.irq";
pub const USES_MSG: &str = "acts.core.msg";
pub const USES_SET: &str = "acts.transform.set";
pub const USES_ACTION: &str = "acts.core.action";
pub const USES_BLOCK: &str = "acts.core.block";
pub const USES_PARALLEL: &str = "acts.core.parallel";
pub const USES_SEQUENCE: &str = "acts.core.sequence";

/// The crate-wide functional-test harness. It lives in [`crate::testing`] —
/// exported to downstream tests behind the `test-utils` feature — and every
/// crate-local test module reaches it through this one path.
pub(crate) use crate::testing::FlowFixture;
