//! The running-transport lifecycle (the runtime face of a transport plugin).
//!
//! A transport plugin binds its listener in [`ActPlugin::start`] and joins
//! its tasks in [`ActPlugin::shutdown`]. Between those two, [`Engine::close`]
//! drives a transport through the graceful sequence:
//!
//! 1. [`RunningTransport::quiesce`] — stop admitting new work; existing
//!    admitted work keeps running.
//! 2. [`RunningTransport::drain`] — wait until admitted work finishes or the
//!    deadline (`[shutdown].drain_timeout_ms`, default 5 s) passes, and
//!    answer a [`TransportDrainReport`] naming what remained.
//! 3. [`ActPlugin::shutdown`] — join the tasks and release the port or
//!    subscription.
//!
//! A transport registers its handle with [`Engine::register_transport`]
//! during `start`; an unregistered transport is closed the old way (the
//! shutdown token is cancelled before `shutdown` runs either way).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

/// The transport drain window when `[shutdown].drain_timeout_ms` does not
/// say otherwise.
pub const DEFAULT_DRAIN_TIMEOUT_MS: u64 = 5_000;

/// The `[shutdown]` config section: how long the engine waits for a
/// transport's admitted work to finish before the transport's tasks are
/// joined regardless.
#[derive(Debug, Clone, Copy, Default, serde::Deserialize)]
pub struct ShutdownConfig {
    pub drain_timeout_ms: Option<u64>,
}

impl ShutdownConfig {
    /// The drain deadline, clamped to 100 ms .. 60 s; omitted or zero
    /// selects the default.
    pub fn timeout(&self) -> Duration {
        let millis = match self.drain_timeout_ms {
            None | Some(0) => DEFAULT_DRAIN_TIMEOUT_MS,
            Some(v) => v.clamp(100, 60_000),
        };
        Duration::from_millis(millis)
    }
}

/// What one transport reports after its drain deadline passed or was met.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportDrainReport {
    /// The transport label (`web`, `grpc`, `nats`).
    pub transport: &'static str,
    /// Admitted work still running when the drain ended.
    pub active_after: usize,
    /// Whether everything had finished before the deadline.
    pub drained: bool,
}

/// A started transport the engine can quiesce and drain before its tasks are
/// joined.
#[async_trait::async_trait]
pub trait RunningTransport: Send + Sync {
    /// The transport label (`web`, `grpc`, `nats`).
    fn transport(&self) -> &'static str;

    /// Stop admitting new work: after this returns, every new request is
    /// refused by the transport itself (503 / `UNAVAILABLE` / an error
    /// reply) without reaching the engine. Admitted work continues.
    async fn quiesce(&self);

    /// Wait for admitted work to finish, up to `deadline`. Answers how much
    /// was still running when the wait ended.
    async fn drain(&self, deadline: Duration) -> TransportDrainReport;
}

/// The admitted-work counter behind a [`RunningTransport`] implementation.
///
/// `enter` is the gate every request handler passes: `None` once quiescing,
/// otherwise a guard whose drop decrements the count. `drain` polls the
/// count to zero (or gives up at the deadline).
pub struct InflightCounter {
    quiescing: AtomicBool,
    active: AtomicUsize,
}

/// One admitted request. Dropping it releases the counter.
pub struct InflightGuard<'a>(&'a InflightCounter);

impl InflightCounter {
    pub fn new() -> Self {
        Self {
            quiescing: AtomicBool::new(false),
            active: AtomicUsize::new(0),
        }
    }

    /// Admit one request: `None` when the transport is quiescing.
    pub fn enter(&self) -> Option<InflightGuard<'_>> {
        if self.quiescing.load(Ordering::Acquire) {
            return None;
        }
        // A quiesce between the check and the increment only makes the count
        // *more* conservative (drain waits out this request), never less.
        self.active.fetch_add(1, Ordering::AcqRel);
        Some(InflightGuard(self))
    }

    /// Stop admitting new requests.
    pub fn quiesce(&self) {
        self.quiescing.store(true, Ordering::Release);
    }

    /// Admitted work still running.
    pub fn active(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }

    /// Whether new requests are refused.
    pub fn is_quiescing(&self) -> bool {
        self.quiescing.load(Ordering::Acquire)
    }

    /// Wait for admitted work to finish, up to `deadline`, and answer the
    /// report.
    pub async fn drain(&self, transport: &'static str, deadline: Duration) -> TransportDrainReport {
        let deadline = tokio::time::Instant::now() + deadline;
        while self.active() > 0 {
            if tokio::time::Instant::now() >= deadline {
                return TransportDrainReport {
                    transport,
                    active_after: self.active(),
                    drained: false,
                };
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        TransportDrainReport {
            transport,
            active_after: 0,
            drained: true,
        }
    }
}

impl Default for InflightCounter {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for InflightGuard<'_> {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gate admits until quiesce, then refuses; drained when admitted
    /// work finishes inside the deadline, and the report names leftovers
    /// when it does not.
    #[tokio::test]
    async fn quiesce_refuses_and_drain_waits_for_admitted_work() {
        let counter = InflightCounter::new();
        let guard = counter.enter().expect("admitted while open");
        assert_eq!(counter.active(), 1);

        counter.quiesce();
        assert!(
            counter.enter().is_none(),
            "a quiesced transport admits nothing"
        );
        assert_eq!(counter.active(), 1, "admitted work is unaffected");

        // The held guard keeps the drain busy past a short deadline: the
        // report names what remained.
        let report = counter.drain("web", Duration::from_millis(30)).await;
        assert_eq!(report.transport, "web");
        assert!(!report.drained);
        assert_eq!(report.active_after, 1);

        drop(guard);
        let report = counter.drain("web", Duration::from_secs(1)).await;
        assert!(report.drained);
        assert_eq!(report.active_after, 0);
    }

    /// A zero-count transport drains immediately.
    #[tokio::test]
    async fn an_idle_transport_drains_at_once() {
        let counter = InflightCounter::new();
        let report = counter.drain("grpc", Duration::from_millis(1)).await;
        assert!(report.drained);
        assert_eq!(report.active_after, 0);
    }
}
