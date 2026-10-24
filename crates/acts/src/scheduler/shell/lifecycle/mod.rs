//! Lifecycle: the process-lifecycle report, the pid existence locks and
//! reclamation.
//!
//! The report is the driver's mirror of the legacy `on_proc` emission: a run's
//! start and terminal, each reported once, best-effort — losing one costs an
//! engine event, never a durable fact. Reclamation is the opposite: the
//! aggregate batch, the row/workdir cleanup and the residue retry are all
//! ordered under the pid's existence lock.

use super::*;

/// Which engine event a run's lifecycle report is: the driver's mirror of the
/// legacy `on_proc` emission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProcessEvent {
    Started,
    Completed,
    Errored,
}

/// The process-lifecycle sink: the engine routes it onto its channel events
/// (`on_start`/`on_complete`/`on_error`).
pub(crate) type LifecycleSink = Box<dyn Fn(&crate::Message, ProcessEvent) + Send + Sync>;

/// Which runs were already reported: a start once, a terminal once.
///
/// `started_at` is the fallback wall-clock anchor for a host with no read
/// model (the kernel's own tests): the projected proc row is the primary
/// source of a run's window, but a lifecycle message must still carry a start
/// and an end when there is no row to read.
#[derive(Default)]
pub(crate) struct ReportedRuns {
    started: std::collections::HashSet<String>,
    finished: std::collections::HashSet<String>,
    started_at: std::collections::HashMap<String, i64>,
}

impl ReportedRuns {
    /// The start instant of `pid`: the projected proc row when it has one,
    /// otherwise the instant this host first reported the run (a host without
    /// a read model), otherwise now. Remembered for the terminal event, whose
    /// message must span the same window.
    fn stamp_start(&mut self, pid: &str, times: Option<(i64, i64)>) -> i64 {
        let now = crate::utils::time::time_millis();
        let start = times
            .map(|(start, _)| start)
            .filter(|start| *start > 0)
            .or_else(|| self.started_at.get(pid).copied())
            .unwrap_or(now);
        self.started_at.insert(pid.to_string(), start);
        start
    }

    /// Stamp a terminal lifecycle message with the run's window. The projected
    /// proc row is authoritative; without one the host's recorded start and
    /// its own clock close the window, so `Message::cost()` is never the
    /// meaningless `0 - 0`.
    fn stamp_terminal(&mut self, message: &mut crate::Message, times: Option<(i64, i64)>) {
        let now = crate::utils::time::time_millis();
        let start = times
            .map(|(start, _)| start)
            .filter(|start| *start > 0)
            .or_else(|| self.started_at.remove(&message.pid))
            .unwrap_or(now);
        let end = times
            .map(|(_, end)| end)
            .filter(|end| *end > 0)
            .unwrap_or(now);
        message.start_time = start;
        message.end_time = end.max(start);
    }
}

impl KernelHost {
    /// Serialize a start and the cleanup of the same pid (see
    /// [`Self::lifecycle_locks`]). A start holds it from the
    /// aggregate-existence check through creation; cleanup holds it from the
    /// authoritative batch through the row/workdir removal.
    pub(crate) async fn lock_lifecycle(
        &self,
        process_id: &str,
    ) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = {
            let mut locks = self.lifecycle_locks.lock();
            locks
                .entry(process_id.to_string())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        lock.lock_owned().await
    }

    /// Mark that a pid's aggregate was deleted and its residue is still being
    /// removed; a start refuses the pid until [`Self::finish_reclaim`].
    pub(crate) fn begin_reclaim(&self, process_id: &str) {
        self.reclaiming.lock().insert(process_id.to_string());
    }

    /// The residue is gone (or the run turned out not to be reclaimable).
    pub(crate) fn finish_reclaim(&self, process_id: &str) {
        self.reclaiming.lock().remove(process_id);
    }

    /// Whether a cleanup of this pid started and has not finished. A start
    /// checks this under the lifecycle lock.
    pub(crate) fn is_reclaiming(&self, process_id: &str) -> bool {
        self.reclaiming.lock().contains(process_id)
    }

    /// The pids whose residue removal is still pending, for the cleanup pass
    /// to clear after a failure whose rows turned out to be gone.
    pub(crate) fn reclaiming_pids(&self) -> Vec<String> {
        self.reclaiming.lock().iter().cloned().collect()
    }

    /// Report one run's lifecycle from a fresh snapshot, at most once per
    /// phase. Mirrors the legacy `on_proc`: a start while the run is alive
    /// (running or pending); otherwise — every execution terminal — an error
    /// for a failed root, a completion for any other terminal, with the
    /// model's `exposes` validated against the run's outputs (a validation
    /// failure is reported as an error, exactly like the legacy handler).
    ///
    /// The root is announced twice on purpose: this sink is the legacy
    /// `on_proc`, while the root's own outbox records (`message.created` with
    /// `kind=workflow`, then its terminal topics) are the legacy `on_task`.
    /// The legacy engine emitted both, so both paths stay — neither replaces
    /// the other, and they carry different fact sets (`process_message` here,
    /// the task payload there).
    pub(in crate::scheduler::shell) async fn report_lifecycle(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    ) {
        let directive = decision::lifecycle(snapshot);
        let times = match directive {
            Some(_) => self.proc_times(&snapshot.process_id).await,
            None => None,
        };
        self.report_lifecycle_decision(snapshot, directive.as_ref(), times);
    }

    /// The run's projected `(start_time, end_time)` when a read model is
    /// attached. The projection has already run for this commit (A5/A6), so
    /// the proc row is current; `None` means no projector, or a row that does
    /// not exist yet (a lifecycle message then falls back to the host's own
    /// clock).
    pub(in crate::scheduler::shell) async fn proc_times(
        &self,
        process_id: &str,
    ) -> Option<(i64, i64)> {
        let projector = self.projection.read().clone()?;
        projector.proc_times(process_id).await.ok().flatten()
    }

    /// Execute the lifecycle directive Core already decided. The shell owns
    /// only the message shape, the sink and the once-per-phase bookkeeping;
    /// *whether* the run is alive or settled (and whether a terminal root is an
    /// error) is [`crate::scheduler::core::decision::lifecycle`].
    ///
    /// `times` is the run's projected window. A projected row is the same
    /// wall-clock fact a `proc:get` reader sees, so `Message::cost()` on the
    /// completion event is the run's real cost; when no row exists (a host
    /// without a read model) the host's own clock supplies the window.
    pub(in crate::scheduler::shell) fn report_lifecycle_decision(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
        directive: Option<&LifecycleDirective>,
        times: Option<(i64, i64)>,
    ) {
        let lifecycle = self.lifecycle.read();
        let Some(sink) = lifecycle.as_ref() else {
            return;
        };
        let Some(root) = root_task(snapshot) else {
            return;
        };
        let Some(directive) = directive else {
            return;
        };
        let mut reported = self.reported.lock();
        match directive {
            LifecycleDirective::Started => {
                if reported.started.insert(snapshot.process_id.clone()) {
                    let mut message = process_message(snapshot, &root);
                    let start = reported.stamp_start(&message.pid, times);
                    message.start_time = start;
                    message.end_time = 0;
                    sink(&message, ProcessEvent::Started);
                }
            }
            LifecycleDirective::Completed => {
                if !reported.finished.insert(snapshot.process_id.clone()) {
                    return;
                }
                let mut message = process_message(snapshot, &root);
                reported.stamp_terminal(&mut message, times);
                // The legacy `on_proc` handler validates the model's
                // `exposes` against the completed outputs; a mismatch is
                // reported as an error on the same completion event.
                match exposes_error(snapshot, &message) {
                    Some(error) => {
                        message.set_err("", &error);
                        sink(&message, ProcessEvent::Errored);
                    }
                    None => sink(&message, ProcessEvent::Completed),
                }
            }
            LifecycleDirective::Errored { code, message } => {
                if !reported.finished.insert(snapshot.process_id.clone()) {
                    return;
                }
                let mut payload = process_message(snapshot, &root);
                reported.stamp_terminal(&mut payload, times);
                payload.set_err(code, message);
                sink(&payload, ProcessEvent::Errored);
            }
        }
    }

    /// Reclaim one finished run's durable state: the aggregate and its
    /// recovery/outbox/ready index documents are deleted in one guarded batch
    /// (see [`AsyncProcessRepository::reclaim`]). The caller is responsible for
    /// the preconditions that live outside the aggregate (channel delivery
    /// rows); this method decides only what the aggregate itself says.
    ///
    /// On a successful reclaim (or an aggregate that is already gone) the
    /// host forgets its in-process bookkeeping for the pid — reported
    /// lifecycle phases, admission slots and projection version claims — so a
    /// later start under the same pid is a fresh run. `Retained` leaves every
    /// fact exactly as it was.
    pub(crate) async fn reclaim_process(&self, process_id: &str) -> Result<ReclaimOutcome> {
        let outcome = self
            .processes
            .reclaim(process_id)
            .await
            .map_err(repository_error)?;
        if !matches!(outcome, ReclaimOutcome::Retained) {
            self.forget_process(process_id);
        }
        Ok(outcome)
    }

    /// Drop the pid-keyed in-process state of a reclaimed run. Called only
    /// once the durable aggregate is gone (or was already gone): the pid is a
    /// new identity from here on, and the next start must not inherit a
    /// reported lifecycle phase, a slot or a projection claim.
    fn forget_process(&self, process_id: &str) {
        self.admissions.lock().abandon(process_id);
        {
            let mut reported = self.reported.lock();
            reported.started.remove(process_id);
            reported.finished.remove(process_id);
        }
        // A handle still waiting on the reclaimed run must not wait forever:
        // its re-read finds no aggregate and reports the PID as gone.
        self.notify_settled(process_id);
        let projector = self.projection.read().clone();
        if let Some(projector) = projector {
            projector.forget_process(process_id);
        }
    }

    /// Clear a reclaimed run's projection tombstone before a new run takes the
    /// pid (see [`projection::RowProjector::forget_process`]).
    pub(in crate::scheduler::shell) fn resume_process(&self, process_id: &str) {
        let projector = self.projection.read().clone();
        if let Some(projector) = projector {
            projector.resume(process_id);
        }
    }
}
