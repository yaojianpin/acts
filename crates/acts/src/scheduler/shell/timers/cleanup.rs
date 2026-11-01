//! The terminal-run cleanup timer: its own interval, batch budget and health.
//! It never shares the operation driver's ready/recovery pass or the
//! message-retry loop, so a slow or failing cleanup cannot delay normal
//! operations or recovery. The loop itself is the kernel host's serve tick
//! ([`TimerGuider::cleanup_tick`]); this file holds the pass.
//!
//! # Why a cycle, not "the first page"
//!
//! A cleanup page used to be the first `cleanup_batch` terminal rows in id
//! order. A run that could not be reclaimed yet — an outbox record still
//! pending because the outbox loop is degraded, a delivery that never settled,
//! an operation awaiting manual reconciliation — sat at the front of that page
//! forever and every later run waited behind it. The sweep also reclaimed at
//! most one page per tick, so a burst of ten thousand finished runs needed
//! ~40 ticks to disappear even when every one of them was reclaimable.
//!
//! The tick now walks a *cycle* over the whole terminal set: the ids are
//! loaded once per cycle, and each tick takes the next slice and drains it
//! until the time budget or the batch bound is reached. A retained run is
//! examined once per cycle and never blocks the rest, and a large backlog
//! drains in one tick instead of one page per tick. The cursor lives in the
//! guider across ticks; the next cycle re-reads the terminal set (which also
//! picks up runs that finished while the previous cycle was walking).
//!
//! The tick also owns its own tokio task (the ready/recovery/outbox ticks stay
//! on the host's serve loop): a pass over thousands of runs takes seconds, and
//! a serve loop waiting behind it would delay every other control loop. The
//! per-pid lifecycle and projection locks still order cleanup against starts
//! and commits, so nothing about the decision changes with the task split.

#[cfg(not(test))]
use tracing::error;

use std::sync::Arc;

use super::TimerGuider;
use crate::scheduler::Runtime;
use crate::scheduler::core::health::CleanupPass;
use crate::store::writebehind;
use crate::{ActError, Result};

/// How long one cleanup tick may keep draining before it yields to the next
/// interval. Cleanup owns its own task (see [`crate::scheduler::Runtime`]), so
/// this budget bounds one background pass, not the kernel serve loop: the
/// rest of the cycle resumes on the next tick. It is deliberately much wider
/// than a liveness slice — a backlog of thousands of finished runs is bulk
/// work, and one page of it used to be all a 15 s tick could do.
#[cfg(not(test))]
const CLEANUP_TICK_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);
/// Batches one cleanup tick may run before yielding, even while the budget
/// remains. This is the ceiling on one pass regardless of how cheap a run is;
/// 64 pages cover 16k runs at the default page size.
#[cfg(not(test))]
const CLEANUP_TICK_BATCHES: usize = 64;

impl TimerGuider {
    /// One terminal-run cleanup tick: walk the current cycle's next slices
    /// until it is exhausted, the tick budget is spent, or the batch bound is
    /// reached. The server's own task calls this; the test build drives
    /// [`sweep_removable`]/[`sweep_cycle`] explicitly instead, because the
    /// reliability cells read a settled run's aggregate as its durable
    /// decision after the run ends.
    #[cfg(not(test))]
    pub(crate) async fn cleanup_tick(&self) {
        {
            // A degraded loop sits this tick out: the store has failed every
            // recent attempt, so another pass now would fail too (and log
            // again) — the guard decides when the next attempt is worth
            // making.
            if !self.timers.cleanup().attempt() {
                return;
            }
            // Load a fresh cycle once the previous one was walked. The load is
            // one indexed scan of the terminal set, never one per batch.
            if self.cleanup_cycle_exhausted() {
                match self.runtime.store().terminal_proc_ids().await {
                    Ok(ids) => self.replace_cleanup_cycle(ids),
                    Err(err) => {
                        error!(error = %err, "terminal cleanup could not list finished runs");
                        self.timers.cleanup().failed(&err);
                        return;
                    }
                }
            }
            let limit = self.runtime.config().cleanup_batch();
            let started = std::time::Instant::now();
            let mut reclaimed = 0usize;
            let mut failure: Option<ActError> = None;
            for _ in 0..CLEANUP_TICK_BATCHES {
                let batch = self.next_cleanup_batch(limit);
                if batch.is_empty() {
                    break;
                }
                let last = batch.len() < limit;
                let page_started = std::time::Instant::now();
                match sweep_pids(&self.runtime, &batch).await {
                    Ok(removed) => {
                        tracing::debug!(
                            page = batch.len(),
                            reclaimed = removed,
                            elapsed_ms = page_started.elapsed().as_millis() as u64,
                            "terminal cleanup page"
                        );
                        reclaimed += removed;
                    }
                    Err(err) => {
                        failure = Some(err);
                        break;
                    }
                }
                if last || started.elapsed() >= CLEANUP_TICK_BUDGET {
                    break;
                }
            }
            match failure {
                Some(err) => {
                    // Every run of the page was attempted: the error names the
                    // runs that failed, not a store that could not be reached.
                    // Health still degrades (the failures are the operator's
                    // to see), but the backoff window is not armed — one bad
                    // run must not slow the cleanup of every later run.
                    error!(error = %err, "terminal cleanup pass isolated per-run failures");
                    self.timers.cleanup().failed_isolated(&err);
                }
                None => {
                    self.timers.cleanup().recovered();
                    let metrics = self.runtime.cleanup_metrics();
                    if metrics.backlog > 0 || reclaimed > 0 {
                        tracing::debug!(
                            backlog = metrics.backlog,
                            oldest_removable_age_ms = metrics.oldest_removable_age_ms,
                            reclaimed,
                            document_bytes_deleted = metrics.document_bytes_deleted,
                            delete_failures = metrics.delete_failures,
                            orphan_indexes_removed = metrics.orphan_indexes_removed,
                            "terminal cleanup"
                        );
                    }
                }
            }
        }
    }
}

/// One terminal-run cleanup pass over the first `cleanup_batch` finished
/// runs: reclaim the durable state of the runs that are ready and then delete
/// their rows and workdirs.
///
/// This is the single-page seam the reliability cells drive directly. The
/// production timer walks the whole terminal set with [`sweep_pids`]; both
/// share [`sweep_rows`], so the decision to reclaim (or retain) one run is the
/// same in both.
#[cfg(test)]
pub(crate) async fn sweep_removable(runtime: &Arc<Runtime>) -> Result<usize> {
    let limit = runtime.config().cleanup_batch();
    for pid in runtime.store().unmarked_settled_procs(limit).await? {
        runtime.workers().refresh_projection(&pid).await;
    }
    let rows = runtime.store().terminal_proc_rows(limit).await?;
    sweep_rows(runtime, rows).await
}

/// Walk a whole cleanup cycle: every id in `pids`, one [`sweep_pids`] page at
/// a time. A page that is entirely retained does not stop the walk — later
/// pages are still examined, which is what "the first page" got wrong.
///
/// The production tick takes its pages from a cursor that persists across
/// ticks; this function is the cycle in one call (and the test seam that
/// proves a retained run cannot starve the runs behind it).
#[cfg(test)]
pub(crate) async fn sweep_cycle(
    runtime: &Arc<Runtime>,
    pids: &[String],
    limit: usize,
) -> Result<usize> {
    let mut reclaimed = 0usize;
    for page in pids.chunks(limit.max(1)) {
        reclaimed += sweep_pids(runtime, page).await?;
    }
    Ok(reclaimed)
}

impl TimerGuider {
    /// The cycle's next slice: take up to `limit` ids and advance the cursor.
    #[cfg(not(test))]
    fn next_cleanup_batch(&self, limit: usize) -> Vec<String> {
        let mut cycle = self.cleanup_cycle.lock();
        let end = cycle.next.saturating_add(limit).min(cycle.ids.len());
        let batch = cycle.ids[cycle.next..end].to_vec();
        cycle.next = end;
        batch
    }
}

/// One page of a cycle: read each run's row, re-derive the `removable` mark of
/// the runs that still carry `false` (a settled run has no later commit to
/// repair it), then reclaim/remove the whole page.
async fn sweep_pids(runtime: &Arc<Runtime>, pids: &[String]) -> Result<usize> {
    let mut rows = Vec::with_capacity(pids.len());
    for pid in pids {
        let Some(row) = runtime.store().procs().find_opt(pid).await? else {
            // The run was removed (or its pid reused and settled again)
            // between the cycle scan and this read; the next cycle re-reads
            // the set.
            continue;
        };
        if row.removable {
            rows.push(row);
            continue;
        }
        // The mark is written by the projection of the run's own commits, and
        // a settled run has none left: its last commit can predate the
        // delivery that settled its outbox records. Re-derive it before the
        // reclaim decision — the proc row alone carries `removable`, so the
        // sweep does not rewrite the run's task and vars rows to decide it.
        runtime.workers().refresh_proc_row(pid).await;
        if let Some(row) = runtime.store().procs().find_opt(pid).await? {
            rows.push(row);
        }
    }
    sweep_rows(runtime, rows).await
}

/// Reclaim and remove one page of finished runs. A run whose aggregate is not
/// reclaimable keeps everything (the pid is not reusable); the next cycle
/// examines it again without blocking the runs after it.
async fn sweep_rows(runtime: &Arc<Runtime>, rows: Vec<crate::store::data::Proc>) -> Result<usize> {
    let now = crate::utils::time::time_millis();
    let mut pass = CleanupPass {
        backlog: rows.len(),
        oldest_removable_age_ms: rows
            .iter()
            .filter(|row| row.removable && row.end_time > 0)
            .map(|row| now.saturating_sub(row.end_time))
            .max(),
        ..CleanupPass::default()
    };
    let mut removed = 0usize;
    let mut failure: Option<ActError> = None;
    for row in rows {
        match cleanup_process(runtime, &row.id, row.removable, &mut pass).await {
            Ok(remove) => {
                if remove {
                    removed += 1;
                }
            }
            Err(error) => {
                // One run's failure must not stop the pass: every other
                // run's cleanup is independent, and this run is retried
                // on the next tick.
                pass.delete_failures += 1;
                tracing::warn!(
                    pid = %row.id,
                    error = %error,
                    "terminal cleanup failed for one process; the next pass retries"
                );
                failure.get_or_insert(error);
            }
        }
    }
    // A failed pass leaves a pid marked until its residue is gone. If the
    // rows disappeared anyway (the batch applied, an operator cleaned up),
    // clear the mark so the pid is not blocked forever.
    for pid in runtime.workers().reclaiming_pids() {
        if let Ok(None) = runtime.store().procs().find_opt(&pid).await {
            runtime.workers().finish_reclaim(&pid);
        }
    }
    runtime.workers().timer_guards().fold_cleanup(&pass);
    match failure {
        Some(error) => Err(error),
        None => Ok(removed),
    }
}

/// Reclaim one process's aggregate and indexes, then report whether its
/// rows may be removed.
///
/// The writer's pid shard is barriered first: a delivery row the transport
/// queued this instant must be visible to the `removable` decision, or the
/// rows would be deleted behind a still-open delivery.
async fn cleanup_process(
    runtime: &Arc<Runtime>,
    pid: &str,
    marked: bool,
    pass: &mut CleanupPass,
) -> Result<bool> {
    // A start and this cleanup are serialized per pid. The lifecycle lock
    // is held from the aggregate decision through the row/workdir
    // removal: `Runtime::start` checks the aggregate under the same lock,
    // so it can never observe "aggregate gone, rows still there" and
    // create a new run whose rows the old cleanup would then delete.
    let _lifecycle = runtime.workers().lock_lifecycle(pid).await;
    // Projection and cleanup are ordered per pid: an in-flight projection
    // of the old run finishes before the aggregate is judged (its rows
    // cannot be written behind the removal), and the tombstone the
    // reclamation leaves blocks every later one.
    let _projection = runtime.workers().lock_projection(pid).await;
    runtime.writer().flush_pid(pid).await?;
    if runtime.store().has_unsettled_deliveries(pid).await? {
        return Ok(false);
    }
    if !marked {
        // The design's step 1: the mark is written before the authoritative
        // batch. It is also what makes an interrupted cleanup retryable —
        // if the aggregate is gone and only rows remain, the mark is the
        // durable evidence that the run was settled when it was cut.
        return Ok(false);
    }
    // Set before the batch: if the batch succeeds and the row removal
    // fails, a start must keep refusing the pid until a later pass
    // finishes the residue. Cleared only after the rows are gone.
    runtime.workers().begin_reclaim(pid);
    let outcome = runtime.workers().reclaim_process(pid).await;
    let (document_bytes, orphan_indexes) = match outcome {
        Ok(crate::scheduler::durable::async_document_store::ReclaimOutcome::Reclaimed {
            document_bytes,
            orphan_indexes,
        })
        | Ok(crate::scheduler::durable::async_document_store::ReclaimOutcome::Absent {
            document_bytes,
            orphan_indexes,
        }) => (document_bytes, orphan_indexes),
        Ok(crate::scheduler::durable::async_document_store::ReclaimOutcome::Retained) => {
            runtime.workers().finish_reclaim(pid);
            tracing::debug!(
                pid,
                "terminal cleanup retained a run for recovery/reconciliation"
            );
            return Ok(false);
        }
        // Leave the mark set: a batch that did land may have left rows.
        Err(error) => return Err(error),
    };
    pass.reclaimed += 1;
    pass.document_bytes += document_bytes;
    pass.orphan_indexes += orphan_indexes as u64;

    // Rows and workdir go while the lifecycle lock is still held; the
    // flush makes the removal durable before a new start can take the pid.
    let removal = match writebehind::queue_removal(&runtime.store(), runtime.writer(), pid).await {
        Ok(()) => runtime.writer().flush_pid(pid).await,
        Err(error) => Err(error),
    };
    if let Err(error) = removal {
        // The `RemoveProc` batch may have applied even though the flush
        // reported a failure. The proc row is deleted atomically with every
        // other row of the process, so its absence proves the removal;
        // keep the mark only when residue may still be there.
        match runtime.store().procs().find_opt(pid).await {
            Ok(None) => {
                runtime.workers().finish_reclaim(pid);
                return Ok(true);
            }
            _ => return Err(error),
        }
    }
    runtime.workers().finish_reclaim(pid);
    Ok(true)
}
