//! A durable handle to one started run.
//!
//! `Executor::proc().start()` returns a [`RunHandle`] instead of a bare pid.
//! The handle is the run's identity (`pid`) plus the runtime that can read it;
//! nothing about it lives only in this process, so a caller that has the pid
//! can rebuild the same handle after a restart with `proc().handle(pid)` and
//! keep waiting.
//!
//! [`RunHandle::wait`] takes its answer from the durable aggregate, never from
//! the in-process event channel:
//!
//! 1. read the aggregate; if the run settled, return its outcome;
//! 2. register a one-shot wakeup for the pid;
//! 3. read the aggregate again — a settle between (1) and (2) is seen here,
//!    so no notification can be missed;
//! 4. otherwise sleep until the wakeup fires and repeat.
//!
//! The event channel (`on_complete`/`on_error`) stays what it was: a
//! notification stream for observability and transports. It is not a
//! correctness source, and a run that finishes before a subscriber registers
//! no longer strands the caller — the aggregate still holds the terminal
//! state.

use crate::{ActError, Result, scheduler::Runtime};

pub use crate::scheduler::RunOutcome;

use std::sync::Arc;

/// A waitable handle to a run, durable because it is just the pid.
///
/// Produced by `Executor::proc().start()`/`start_from_model()`, or rebuilt
/// from a pid with `Executor::proc().handle()` (the restart path). Cloning the
/// handle is cloning the identity, not the run.
#[derive(Clone)]
pub struct RunHandle {
    runtime: Arc<Runtime>,
    pid: String,
}

impl RunHandle {
    pub(crate) fn new(runtime: &Arc<Runtime>, pid: String) -> Self {
        Self {
            runtime: runtime.clone(),
            pid,
        }
    }

    /// The run's durable id.
    pub fn pid(&self) -> &str {
        &self.pid
    }

    /// Wait for the run to settle, and answer what it settled as.
    ///
    /// Returns `Completed`, `Errored` (with the failed root's durable
    /// `error.code`/`error.message`) or `Terminated` (aborted/cancelled/skipped/
    /// removed root). The wait is restart-proof: the answer comes from the
    /// aggregate, and a waiter registered after the terminal commit returns on
    /// the immediate re-read.
    ///
    /// Fails when the pid's durable record is gone — an id this runtime never
    /// had, or a run the A11 reclamation already removed. A reclaimed run has
    /// no terminal state left to report by design; capture the outcome before
    /// its rows/aggregate are swept if it must be retained.
    pub async fn wait(&self) -> Result<RunOutcome> {
        loop {
            if let Some(outcome) = self.outcome().await? {
                return Ok(outcome);
            }
            // Register before the re-check: `enable` puts this waiter on the
            // pid's list, so a settle between the first read and here is
            // either seen by the re-read below or wakes this future.
            let waiter = self.runtime.kernel().run_waiter(&self.pid);
            let notified = waiter.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(outcome) = self.outcome().await? {
                return Ok(outcome);
            }
            notified.await;
        }
    }

    /// The durable outcome, or `None` while the run is in flight.
    async fn outcome(&self) -> Result<Option<RunOutcome>> {
        let Some(snapshot) = self.runtime.kernel().snapshot(&self.pid).await? else {
            return Err(ActError::Runtime(format!(
                "process '{}' is not present (never started here, or already reclaimed)",
                self.pid
            )));
        };
        Ok(crate::scheduler::run_outcome(&snapshot))
    }
}

impl std::fmt::Debug for RunHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunHandle").field("pid", &self.pid).finish()
    }
}

impl std::fmt::Display for RunHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.pid)
    }
}

/// A handle serializes as the pid it names, so `proc:start` keeps answering
/// the pid string it always did.
impl serde::Serialize for RunHandle {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.pid)
    }
}
