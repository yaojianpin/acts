//! Functional scheduler cells: steps, branches, acts, vars and messages.
//!
//! These are the pre-kernel scheduler tests (`step/*`, `branch/*`, `act/*`,
//! `message`, `vars`, `proc`, `scher`), migrated to the kernel engine. Each
//! cell keeps the legacy goal but observes the kernel's own surfaces: the
//! aggregate snapshot for execution states, the engine's channel for the
//! messages a run announces, and the projected rows for the read model.
//!
//! [`FlowFixture`] is the shared harness (it lives in [`crate::testing`], so a
//! package or plugin crate enables the `test-utils` feature and uses the same
//! one). `deploy` prepares an engine and its model, `start` runs it, awaits
//! (`wait_created`, `wait_message`, `wait_completed`, `wait_state`) do the
//! synchronization, `next`/`submit`/`skip`/`abort`/`error`/`back`/`cancel`
//! serve the client's action on the message that named the act, and
//! `state`/`outputs`/`messages_of` are the assertions. A cell therefore reads
//! as the behavior it pins instead of as engine plumbing.
//!
//! The kernel never reclaims a run inside the test build (the terminal-cleanup
//! timer is disabled there), so a settled run's aggregate stays readable after
//! its completion event — the same durable evidence the legacy resident
//! process held in memory.

pub(crate) mod act;
pub(crate) mod branch;
pub(crate) mod candidates;
pub(crate) mod catch;
pub(crate) mod graph;
pub(crate) mod join;
pub(crate) mod message;
pub(crate) mod outputs;
pub(crate) mod rules;
pub(crate) mod runtime;
pub(crate) mod step;
pub(crate) mod subflow;
pub(crate) mod timeout;
pub(crate) mod vars;

pub(crate) use crate::testing::{FlowFixture, Terminal, auto_workflow, irq_workflow};
