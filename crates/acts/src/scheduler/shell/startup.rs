//! The runtime's start barrier.
//!
//! A runtime is constructed complete but not yet *started*: its ports are
//! attached by [`crate::scheduler::Runtime::create`], and the driver must not
//! reach the store until the engine's own registrations (snapshots, plugin
//! packages, built-in packages and their runtime instances) are committed.
//! Until then a recovered operation would be planned against a half-built
//! registry: a missing runtime instance fails the task, a missing definition
//! changes the imported model's `auto` facts, and recovery would read an
//! unregistered package as "not idempotent" and strand the run for manual
//! reconciliation.
//!
//! `Runtime::new` builds an **open** gate: an embedder (or any test) that
//! constructs the runtime directly drives it immediately, as before. The
//! builder's start transaction builds a **held** gate (`Runtime::new_gated`)
//! and opens it at the end of `Engine::initialize`; a failed start closes the
//! runtime instead, and the driver leaves through the shutdown token without
//! ever touching the store.
//!
//! The gate is one-shot and monotonic: it never closes again, and `open` is
//! idempotent.

use tokio::sync::watch;

/// A one-shot startup gate (see the module docs).
#[derive(Clone)]
pub(crate) struct StartupGate {
    sender: watch::Sender<bool>,
    receiver: watch::Receiver<bool>,
}

impl StartupGate {
    /// A gate that is already open: the driver starts immediately.
    pub(crate) fn open() -> Self {
        Self::new(true)
    }

    /// A gate held until [`Self::open_now`].
    pub(crate) fn held() -> Self {
        Self::new(false)
    }

    fn new(open: bool) -> Self {
        let (sender, receiver) = watch::channel(open);
        Self { sender, receiver }
    }

    /// Release every waiter. Idempotent; the gate never closes again.
    pub(crate) fn open_now(&self) {
        let open = *self.receiver.borrow();
        if !open {
            self.sender.send_replace(true);
        }
    }

    /// Whether the gate is open. The driver's callers never need the answer —
    /// `wait` is the protocol — so this is a test observation only.
    #[cfg(test)]
    pub(crate) fn is_open(&self) -> bool {
        *self.receiver.borrow()
    }

    /// Resolve once the gate is open. Cancellation-safe: the driver wraps it
    /// in a `select!` with its shutdown token, so a close before the gate
    /// opens exits without running a single pass.
    pub(crate) async fn wait(&self) {
        let mut receiver = self.receiver.clone();
        // A `watch` receiver keeps the current value, so a gate opened between
        // the check and the await is still observed — no lost wakeup.
        loop {
            let open = *receiver.borrow_and_update();
            if open {
                return;
            }
            if receiver.changed().await.is_err() {
                // The gate's only long-lived holder is the host; a dropped
                // sender means the runtime is gone and the waiter is being
                // torn down with it.
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn an_open_gate_passes_at_once() {
        let gate = StartupGate::open();
        assert!(gate.is_open());
        tokio::time::timeout(Duration::from_millis(50), gate.wait())
            .await
            .expect("an open gate must not wait");
    }

    #[tokio::test]
    async fn a_held_gate_waits_until_opened_and_open_is_idempotent() {
        let gate = StartupGate::held();
        assert!(!gate.is_open());
        assert!(
            tokio::time::timeout(Duration::from_millis(50), gate.wait())
                .await
                .is_err(),
            "a held gate must not pass"
        );
        gate.open_now();
        gate.open_now();
        assert!(gate.is_open());
        tokio::time::timeout(Duration::from_millis(50), gate.wait())
            .await
            .expect("an opened gate must release the waiter");
    }
}
