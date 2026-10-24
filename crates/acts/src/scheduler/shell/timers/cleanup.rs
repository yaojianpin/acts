//! The terminal-run cleanup timer: its own interval, batch budget and health.
//! It never shares the operation driver's ready/recovery pass or the
//! message-retry loop, so a slow or failing cleanup cannot delay normal
//! operations or recovery. The loop itself is the kernel host's serve tick
//! ([`TimerGuider::cleanup_tick`]); this file holds the pass.

#[cfg(not(test))]
use tracing::error;

use std::sync::Arc;

use super::TimerGuider;
use crate::scheduler::Runtime;
use crate::scheduler::core::health::CleanupPass;
use crate::store::writebehind;
use crate::{ActError, Result};

impl TimerGuider {
    /// One terminal-run cleanup pass: reclaims finished runs' aggregates,
    /// rows and work directories. Under test this is a no-op — the
    /// reliability cells drive `sweep_removable` explicitly there, because
    /// they read a settled run's aggregate as its durable decision after the
    /// run ends.
    pub(crate) async fn cleanup_tick(&self) {
        #[cfg(test)]
        {
            let _ = self;
        }
        #[cfg(not(test))]
        {
            // A degraded loop sits this tick out: the store has failed every
            // recent attempt, so another pass now would fail too (and log
            // again) — the guard decides when the next attempt is worth
            // making.
            if !self.timers.cleanup().attempt() {
                return;
            }
            match sweep_removable(&self.runtime).await {
                Ok(_) => {
                    self.timers.cleanup().recovered();
                    let metrics = self.runtime.cleanup_metrics();
                    if metrics.backlog > 0 {
                        tracing::debug!(
                            backlog = metrics.backlog,
                            oldest_removable_age_ms = metrics.oldest_removable_age_ms,
                            reclaimed = metrics.reclaimed,
                            document_bytes_deleted = metrics.document_bytes_deleted,
                            delete_failures = metrics.delete_failures,
                            orphan_indexes_removed = metrics.orphan_indexes_removed,
                            "terminal cleanup"
                        );
                    }
                }
                Err(err) => {
                    error!(error = %err, "terminal cleanup pass failed");
                    self.timers.cleanup().failed(&err);
                }
            }
        }
    }
}

/// One terminal-run cleanup pass: reclaim the durable state of finished
/// runs and then delete their rows and workdirs.
///
/// A settled run's `removable` mark is re-derived first. The mark is
/// written by the projection of the run's own commits, and a settled run
/// has none left: its last commit can predate the delivery that settled
/// its outbox records (the delivery pass refreshes the row only for the
/// runs it delivered for, and nothing repairs a settled run's row), so a
/// terminal row that still reads not-removable would otherwise keep the
/// run's rows — and the directory swept with them — forever.
///
/// The authoritative cut is the aggregate deletion (one guarded batch:
/// aggregate + recovery + outbox + ready indexes). Rows and the workdir
/// are then removed idempotently. A delivery row still open, a
/// non-retired operation, an undelivered/unclassified outbox record, or
/// `EffectUnknown`/`Quarantined` keeps the aggregate and every row: the
/// pid is not reusable. An aggregate already gone (an interrupted
/// cleanup) lets the retry remove only its marked rows.
pub(crate) async fn sweep_removable(runtime: &Arc<Runtime>) -> Result<usize> {
    let limit = runtime.config().cleanup_batch();
    for pid in runtime.store().unmarked_settled_procs(limit).await? {
        runtime.kernel().refresh_projection(&pid).await;
    }
    let rows = runtime.store().terminal_proc_rows(limit).await?;
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
    for pid in runtime.kernel().reclaiming_pids() {
        if let Ok(None) = runtime.store().procs().find_opt(&pid).await {
            runtime.kernel().finish_reclaim(&pid);
            runtime.router().forget(&pid);
        }
    }
    runtime.timers().fold_cleanup(&pass);
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
    let _lifecycle = runtime.kernel().lock_lifecycle(pid).await;
    // Projection and cleanup are ordered per pid: an in-flight projection
    // of the old run finishes before the aggregate is judged (its rows
    // cannot be written behind the removal), and the tombstone the
    // reclamation leaves blocks every later one.
    let _projection = runtime.kernel().lock_projection(pid).await;
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
    runtime.kernel().begin_reclaim(pid);
    let outcome = runtime.kernel().reclaim_process(pid).await;
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
            runtime.kernel().finish_reclaim(pid);
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
                runtime.kernel().finish_reclaim(pid);
                runtime.router().forget(pid);
                return Ok(true);
            }
            _ => return Err(error),
        }
    }
    runtime.router().forget(pid);
    runtime.kernel().finish_reclaim(pid);
    Ok(true)
}
