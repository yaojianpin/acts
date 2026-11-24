//! Outbox delivery: the fence, the claim state machine and the retry pass.
//!
//! The capability is the transport trait (contract) and the kernel's own
//! implementations in one module. A record is delivered at-least-once: the
//! normal path applies its claim and the transport's outcome in one in-memory
//! sequence before the one batch commit; the retry pass persists the claim
//! first, so a concurrent worker cannot deliver the same record under the
//! same fence.
//!
//! The transport reports facts only; retry timing and quarantine policy
//! remain the kernel's decisions. The serving implementation is the
//! message forwarder (`caps::message::MessageForwarder`); the shadow
//! transport below exists only in a test build, and a host with no
//! transport attached leaves records `Pending`.

use super::Capability;
use crate::ActError;
use crate::scheduler::core::outbox::outbox_repository::OutboxPageFailure;
use crate::scheduler::shell::*;

/// What one outbox retry pass did.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct OutboxPassReport {
    /// Records whose delivery settled (or was confirmed) in the pass.
    pub(crate) delivered: usize,
    /// Index entries the pass examined, including damaged ones that produced
    /// no record. This is what the pass's own budget bounds.
    pub(crate) examined: usize,
    /// Orphan index documents the page removed because their aggregate was
    /// gone. Resolved residue, not a failure.
    pub(crate) orphans_removed: usize,
    /// Distinct pids the pass isolated because their index or aggregate
    /// could not be used. Non-zero keeps the loop's health degraded without
    /// arming its backoff window.
    pub(crate) poisoned: usize,
}

/// Result of one external delivery attempt. The transport reports facts only;
/// retry timing and quarantine policy remain application decisions.
///
/// REGISTERED EXCEPTION: the attached transport in production is the message
/// forwarder, which only ever reports `Delivered`, so
/// `RetryAt`/`OutcomeUnknown`/`Quarantine` are never constructed in-tree — yet
/// the host's dispatch handles all four exhaustively and turns each into a
/// durable `OutboxEvent`. The enum is the transport contract those handlers
/// exist for; deleting the three would delete the engine's retry/quarantine
/// handling with them.
#[allow(dead_code)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DeliveryOutcome {
    Delivered {
        receipt: crate::scheduler::core::outbox::records::OutboxReceipt,
    },
    RetryAt {
        available_at_ms: i64,
        error: String,
    },
    OutcomeUnknown {
        reason: String,
    },
    Quarantine {
        reason: String,
    },
}

/// Delivers one durable outbox record.
///
/// The host awaits this call. A durable transport (the serving message
/// forwarder) uses the await to make the next layer's rows exist; a test
/// transport may complete immediately. An implementation that needs mutable
/// state keeps it behind its own interior mutability, so a port handle can be
/// shared.
#[async_trait::async_trait]
pub(crate) trait OutboxTransport: Capability {
    async fn deliver(&self, record: &OutboxRecord) -> DeliveryOutcome;
}

/// The fence one delivery attempt uses. The normal path applies it only in
/// memory before its batch confirmation; the retry pass persists it first, so
/// a concurrent worker cannot deliver the same record under the same fence.
fn delivery_claim(record: &OutboxRecord, now: i64) -> DeliveryClaim {
    DeliveryClaim {
        owner: OWNER.into(),
        fence: record.last_fence + 1,
        lease_until_ms: now + DELIVERY_LEASE_MS,
        heartbeat_revision: 0,
    }
}

/// The state-machine event one transport outcome becomes. `Quarantine` is
/// valid from `Pending` directly; every other outcome needs the claim first.
fn outcome_event(claim: &DeliveryClaim, outcome: DeliveryOutcome, now: i64) -> OutboxEvent {
    match outcome {
        DeliveryOutcome::Delivered { receipt } => OutboxEvent::ConfirmDelivered {
            claim: claim.clone(),
            receipt,
            now_ms: now,
        },
        DeliveryOutcome::RetryAt {
            available_at_ms,
            error,
        } => OutboxEvent::Reschedule {
            claim: claim.clone(),
            available_at_ms,
            error,
            now_ms: now,
        },
        DeliveryOutcome::OutcomeUnknown { reason } => OutboxEvent::MarkDeliveryUnknown {
            claim: claim.clone(),
            reason,
            now_ms: now,
        },
        DeliveryOutcome::Quarantine { reason } => OutboxEvent::Quarantine { reason },
    }
}

/// The in-memory event sequence a normal-path delivery applies before the one
/// batch commit: claim, then the transport's outcome.
fn delivery_events(claim: &DeliveryClaim, outcome: DeliveryOutcome, now: i64) -> Vec<OutboxEvent> {
    let event = outcome_event(claim, outcome, now);
    match event {
        OutboxEvent::Quarantine { .. } => vec![event],
        _ => vec![
            OutboxEvent::Claim {
                claim: claim.clone(),
                now_ms: now,
            },
            event,
        ],
    }
}

/// The kernel's **test-only** outbox transport: it logs each record and
/// confirms it without sending anything. The kernel's own tests and the P2
/// parity harness install it explicitly (see `Workers::new`); the serving
/// runtime attaches [`message::MessageForwarder`] instead, and `#[cfg(test)]`
/// keeps this type out of a serving build entirely — a production runtime
/// cannot construct it, let alone default to it.
#[cfg(test)]
pub(crate) struct ShadowTransport;

#[cfg(test)]
impl Capability for ShadowTransport {
    fn key(&self) -> &str {
        "shadow.delivery"
    }
}

#[cfg(test)]
#[async_trait::async_trait]
impl OutboxTransport for ShadowTransport {
    async fn deliver(
        &self,
        record: &crate::scheduler::core::outbox::records::OutboxRecord,
    ) -> DeliveryOutcome {
        tracing::debug!(
            pid = %record.id.process_id,
            key = %record.id.key,
            topic = %record.topic,
            "kernel outbox delivered (shadow)"
        );
        DeliveryOutcome::Delivered {
            receipt: "shadow".into(),
        }
    }
}
/// Whether every `Delivered` record in `snapshot` carries proof the next
/// layer exists. Cleanup calls this before deleting a run's rows: a delivered
/// record whose canonical message is missing (or differs under the same id)
/// is not settled, and a historical `shadow` receipt is known never to have
/// sent anything.
pub(crate) async fn delivered_receipts_verified(
    store: &crate::store::Store,
    snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
) -> crate::Result<bool> {
    use crate::scheduler::core::outbox::records::{OutboxReceipt, OutboxState};

    for record in snapshot.outbox.values() {
        let OutboxState::Delivered { receipt } = &record.state else {
            continue;
        };
        match receipt {
            OutboxReceipt::DurableMessage(receipt) => {
                let Some(message) = store.messages().find(&receipt.message_id).await? else {
                    tracing::error!(
                        pid = %record.id.process_id,
                        key = %record.id.key,
                        message_id = %receipt.message_id,
                        "delivered outbox record has no canonical message; refusing cleanup"
                    );
                    return Ok(false);
                };
                if message.content_hash() != receipt.payload_hash {
                    tracing::error!(
                        pid = %record.id.process_id,
                        key = %record.id.key,
                        message_id = %receipt.message_id,
                        "canonical message content does not match its outbox receipt; refusing cleanup"
                    );
                    return Ok(false);
                }
            }
            OutboxReceipt::Legacy(value) => {
                if value == "shadow" {
                    tracing::warn!(
                        pid = %record.id.process_id,
                        key = %record.id.key,
                        "shadow-delivered outbox record cannot be cleaned; it was never sent"
                    );
                    return Ok(false);
                }
            }
        }
    }
    Ok(true)
}

impl Workers {
    /// Deliver the records one operation just committed and persist every
    /// outcome in **one** follow-up aggregate commit. This is the normal path
    /// (A1b): the plan commit wrote the records `Pending`, the read model was
    /// projected from that commit, and only now does the transport see them.
    /// The confirmation batch is one guarded replace — never one claim and one
    /// confirm per record — so a many-record operation still pays one extra
    /// commit.
    ///
    /// A lost guard applies nothing: the records stay `Pending`, the outbox
    /// pass redelivers them (at-least-once), and the run stays unreclaimable
    /// until every record is `Delivered`.
    pub(in crate::scheduler::shell) async fn deliver_committed_outbox(
        &self,
        loaded: &mut crate::scheduler::durable::document_store::LoadedProcess,
        ids: &[crate::scheduler::core::outbox::records::OutboxId],
    ) {
        if ids.is_empty() {
            return;
        }
        // No transport attached is not "nothing to send": the records stay
        // `Pending` for the transport the serving assembly installs before the
        // driver starts. Confirming them here would be the shadow bug.
        let Some(transport) = self.caps.read().delivery.clone() else {
            return;
        };
        let process_id = loaded.snapshot.process_id.clone();
        let snapshot = &loaded.snapshot;
        let now = SystemClock.now_ms();
        let mut confirmations = Vec::with_capacity(ids.len());
        {
            for id in ids {
                let Some(record) = snapshot.outbox.get(id) else {
                    continue;
                };
                // Only a record this operation left `Pending` is ours to
                // deliver. A concurrent pass may have claimed or settled it
                // between the commit and this pass; that worker owns it now.
                if !matches!(
                    record.state,
                    crate::scheduler::core::outbox::records::OutboxState::Pending { .. }
                ) {
                    continue;
                }
                let outcome = transport.deliver(record).await;
                let claim = delivery_claim(record, now);
                confirmations.push(
                    crate::scheduler::core::outbox::outbox_repository::OutboxConfirmation {
                        id: id.clone(),
                        expected_revision: record.revision,
                        events: delivery_events(&claim, outcome, now),
                    },
                );
            }
        }
        if confirmations.is_empty() {
            return;
        }
        match self
            .processes
            .confirm_outbox_batch(loaded, &confirmations)
            .await
        {
            Ok(outcome) => {
                // A rescheduled record whose retry is due now must not wait for
                // the tick; the pass is the retry net, not the normal path.
                let due_now = outcome.confirmed.iter().any(|record| {
                    matches!(
                        record.state,
                        crate::scheduler::core::outbox::records::OutboxState::Pending {
                            available_at_ms
                        } if available_at_ms <= now
                    )
                });
                if due_now {
                    self.deliver.notify_one();
                }
            }
            Err(error) => {
                tracing::warn!(
                    pid = %process_id,
                    error = %error,
                    "outbox confirmation lost; the retry pass will redeliver"
                );
                self.deliver.notify_one();
            }
        }
    }

    /// Deliver one already-loaded record: claim it with a fenced claim, hand
    /// it to the transport, then persist the reported outcome. After the
    /// transport ran, only its explicit `RetryAt` proves no external effect —
    /// everything else the transport cannot classify stays `DeliveryUnknown`.
    /// The transport lock is taken only for the synchronous `deliver` call,
    /// never across an await.
    ///
    /// `Ok(true)` means the transport confirmed delivery; `Ok(false)` means the
    /// claim was lost or the record was no longer dispatchable.
    async fn dispatch_outbox_record(&self, record: OutboxRecord, now: i64) -> Result<bool> {
        // Nothing to hand the record to: leave it `Pending`, unclaimed. A
        // claim here would be retry accounting without a transport — the
        // shadow bug in slow motion.
        let Some(transport) = self.caps.read().delivery.clone() else {
            return Ok(false);
        };
        let claim = delivery_claim(&record, now);
        // A4: the claim and the outcome are two guarded aggregate writes of
        // this record's pid, so each takes the command lane — never across the
        // transport's synchronous call, exactly like an effect. The record's
        // revision stays the fence against a writer the lane cannot see.
        let lane = self.lock_command_lane(&record.id.process_id).await;
        let claimed = match self
            .processes
            .evolve_outbox(
                &record.id,
                record.revision,
                OutboxEvent::Claim {
                    claim: claim.clone(),
                    now_ms: now,
                },
            )
            .await
        {
            Ok(claimed) => claimed,
            Err(error) => {
                // A concurrent worker claimed it first; skip without failing
                // the pass.
                tracing::debug!(record = ?record.id, error = %error, "delivery claim lost");
                return Ok(false);
            }
        };
        drop(lane);
        let event = {
            let outcome = transport.deliver(&claimed).await;
            outcome_event(&claim, outcome, now)
        };
        let delivered = matches!(&event, OutboxEvent::ConfirmDelivered { .. });
        // A4: the outcome write is a second command of the same pid — the
        // lane is retaken for it, after the transport's synchronous call.
        let _lane = self.lock_command_lane(&record.id.process_id).await;
        self.processes
            .evolve_outbox(&record.id, claimed.revision, event)
            .await
            .map_err(runtime)?;
        Ok(delivered)
    }

    /// One outbox delivery pass over the attached transport: claim every due
    /// record with a fenced claim, hand it to the transport, then persist the
    /// reported outcome.
    #[cfg(test)]
    pub(crate) async fn dispatch_outbox(&self) -> Result<usize> {
        self.dispatch_outbox_batch(PAGE_SIZE).await
    }

    /// The bounded outbox retry pass (A10). At most `budget` records are
    /// delivered per round; a larger backlog resumes at the next record on the
    /// loop's next interval. The cursor is in-memory scheduling state only —
    /// records remain `Pending` in the aggregate, so losing it (restart) just
    /// restarts the pass.
    pub(crate) async fn dispatch_outbox_batch(&self, budget: usize) -> Result<usize> {
        let report = self.dispatch_outbox_pass(budget).await?;
        tracing::trace!(
            delivered = report.delivered,
            examined = report.examined,
            orphans_removed = report.orphans_removed,
            poisoned = report.poisoned,
            "outbox pass finished"
        );
        if report.poisoned > 0 {
            return Err(ActError::Runtime(format!(
                "{} outbox process(es) isolated; the remaining records were delivered",
                report.poisoned
            )));
        }
        Ok(report.delivered)
    }

    /// One outbox retry pass with per-pid isolation.
    ///
    /// A page never fails the pass for one process's damage: the pid is
    /// recorded ([`Workers::outbox_poisoned`]), its index is queued for the
    /// derived-index repair when the aggregate is readable, and every other
    /// pid's records are still delivered. The caller keeps the loop's health
    /// degraded on a non-zero [`OutboxPassReport::poisoned`] but does not arm
    /// the backoff window, so the healthy work keeps flowing.
    pub(crate) async fn dispatch_outbox_pass(&self, budget: usize) -> Result<OutboxPassReport> {
        let now = SystemClock.now_ms();
        let budget = budget.max(1);
        let mut delivered = 0usize;
        let mut processed = 0usize;
        let mut examined = 0usize;
        let mut orphans_removed = 0usize;
        let mut deferred_records = 0u64;
        let mut settled: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut attempted: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut failures: Vec<OutboxPageFailure> = Vec::new();
        // Pids whose rows this pass verified current. The normal path already
        // projected before it delivered; this pass also serves records a
        // failed/withheld projection left `Pending` (and records another
        // instance committed), so it re-projects the run before any external
        // consumer can see the message.
        let mut projected: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        // Pids whose projection failed in this pass: their records stay
        // `Pending`, and retrying the projection for every record of the run
        // would only multiply a store outage.
        let mut deferred: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut cursor = self.outbox_cursor.lock().clone();
        let mut last_processed = cursor.clone();
        let mut first_page = true;
        let mut wrapped = false;
        'pass: loop {
            let remaining = budget
                .saturating_sub(processed)
                .min(budget.saturating_sub(examined))
                .max(1);
            let page = self
                .processes
                .dispatchable_outbox_page(now, cursor.as_ref(), PAGE_SIZE.min(remaining))
                .await
                .map_err(runtime)?;
            examined += page.examined;
            orphans_removed += page.orphans_removed;
            attempted.extend(page.attempted.iter().cloned());
            failures.extend(page.failures.iter().cloned());
            if first_page {
                first_page = false;
                let mut metrics = self.outbox_metrics.lock();
                metrics.observed = page.observed;
                metrics.backlog = page.due;
                metrics.oldest_due_age_ms = page
                    .oldest_due_at_ms
                    .map(|at| now.saturating_sub(at).max(0));
                metrics.passes += 1;
            }
            if page.records.is_empty() {
                // The page examined index entries but produced no deliverable
                // record (missing aggregate/record, stale revision, all
                // deferred). Advance to the last entry it examined even
                // though no record survived: the cursor names examined work,
                // so the next page is reachable and a damaged prefix cannot
                // starve the entries behind it.
                if let Some(next) = page.next {
                    cursor = Some(next.clone());
                    last_processed = Some(OutboxCursor(next.0.clone()));
                    if examined >= budget {
                        break 'pass;
                    }
                    continue;
                }
                // A cursor past the last due record only means the pass
                // wrapped around: start once from the beginning before
                // deciding the backlog is empty.
                if cursor.is_some() && !wrapped {
                    cursor = None;
                    last_processed = None;
                    wrapped = true;
                    continue;
                }
                cursor = None;
                break;
            }
            for record in page.records {
                if processed >= budget {
                    cursor = last_processed.clone();
                    break 'pass;
                }
                last_processed = Some(OutboxCursor(record.id.clone()));
                let process_id = record.id.process_id.clone();
                if deferred.contains(&process_id) {
                    processed += 1;
                    deferred_records += 1;
                    continue;
                }
                if !projected.contains(&process_id) {
                    if !self.repair_projection(&process_id).await {
                        // The rows are still behind; leave every record of
                        // this run `Pending` for the next pass.
                        deferred.insert(process_id.clone());
                        processed += 1;
                        deferred_records += 1;
                        continue;
                    }
                    projected.insert(process_id.clone());
                }
                processed += 1;
                if self.dispatch_outbox_record(record, now).await? {
                    delivered += 1;
                    settled.insert(process_id);
                }
            }
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        *self.outbox_cursor.lock() = cursor;
        {
            let mut metrics = self.outbox_metrics.lock();
            metrics.delivered += delivered as u64;
            metrics.deferred += deferred_records;
            metrics.examined += examined as u64;
            metrics.bad_index += failures.len() as u64;
            metrics.orphans_removed += orphans_removed as u64;
        }
        // The deliveries that settled decide the read model's `removable`
        // flag: a run that finished before its records were delivered only
        // hands its rows (and directory) to the sweeper once they are.
        for process_id in &settled {
            self.refresh_proc_row(process_id).await;
        }
        // Per-pid damage is recorded, logged, and handed to the repair path —
        // never swallowed and never allowed to stop the rest of the pass.
        self.record_outbox_health(&attempted, &failures);
        let poisoned_now = self.outbox_poisoned();
        if !poisoned_now.is_empty() {
            tracing::warn!(
                poisoned = poisoned_now.len(),
                processes = ?poisoned_now
                    .iter()
                    .map(|(pid, entry)| (pid.as_str(), entry.attempts))
                    .collect::<Vec<_>>(),
                "outbox processes are poisoned; the pass continues with the healthy ones and retries these"
            );
        }
        for failure in &failures {
            tracing::warn!(
                pid = %failure.process_id,
                repairable = failure.repairable,
                error = %failure.error,
                "outbox process isolated; the pass continues with the other processes"
            );
            if failure.repairable {
                match self
                    .processes
                    .repair_ready(&failure.process_id, now, self.timer_poll_ms)
                    .await
                {
                    Ok(true) => tracing::info!(
                        pid = %failure.process_id,
                        "outbox index rebuilt from the aggregate; the next pass re-reads it"
                    ),
                    Ok(false) => {}
                    Err(error) => tracing::warn!(
                        pid = %failure.process_id,
                        error = %error,
                        "outbox index repair failed; the pid stays poisoned and is retried"
                    ),
                }
            }
        }
        let poisoned = failures
            .iter()
            .map(|failure| failure.process_id.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        Ok(OutboxPassReport {
            delivered,
            examined,
            orphans_removed,
            poisoned,
        })
    }
}

#[cfg(test)]
mod receipt_tests {
    use super::*;
    use crate::scheduler::core::outbox::records::{
        DurableMessageReceipt, OutboxId, OutboxReceipt, OutboxRecord, OutboxState,
    };
    use crate::store::MemoryStore;
    use std::sync::Arc;

    fn snapshot_with(
        state: OutboxState,
    ) -> crate::scheduler::core::commit::aggregate::ProcessSnapshot {
        let mut snapshot =
            crate::scheduler::core::commit::aggregate::ProcessSnapshot::empty("p1").unwrap();
        let id = OutboxId {
            process_id: "p1".into(),
            key: "k".into(),
        };
        snapshot.outbox.insert(
            id.clone(),
            OutboxRecord {
                id,
                topic: "message.created".into(),
                payload: "{}".into(),
                state,
                attempt: 1,
                last_fence: 1,
                revision: 1,
            },
        );
        snapshot
    }

    #[tokio::test]
    async fn a_verified_durable_receipt_passes() {
        let store = crate::store::Store::new(Arc::new(MemoryStore::new()));
        let message = crate::data::Message {
            id: "m1".into(),
            pid: "p1".into(),
            ..crate::data::Message::default()
        };
        store.messages().create(&message).await.unwrap();
        let snapshot = snapshot_with(OutboxState::Delivered {
            receipt: OutboxReceipt::DurableMessage(DurableMessageReceipt {
                message_id: message.id.clone(),
                payload_hash: message.content_hash(),
                delivery_count: 0,
                delivery_hash: 0,
            }),
        });
        assert!(
            delivered_receipts_verified(&store, &snapshot)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn a_missing_or_mismatched_message_blocks_cleanup() {
        let store = crate::store::Store::new(Arc::new(MemoryStore::new()));
        let snapshot = snapshot_with(OutboxState::Delivered {
            receipt: OutboxReceipt::DurableMessage(DurableMessageReceipt {
                message_id: "missing".into(),
                payload_hash: 1,
                delivery_count: 1,
                delivery_hash: 1,
            }),
        });
        assert!(
            !delivered_receipts_verified(&store, &snapshot)
                .await
                .unwrap()
        );

        let message = crate::data::Message {
            id: "m1".into(),
            pid: "p1".into(),
            ..crate::data::Message::default()
        };
        store.messages().create(&message).await.unwrap();
        let snapshot = snapshot_with(OutboxState::Delivered {
            receipt: OutboxReceipt::DurableMessage(DurableMessageReceipt {
                message_id: message.id.clone(),
                payload_hash: message.content_hash() ^ 1,
                delivery_count: 0,
                delivery_hash: 0,
            }),
        });
        assert!(
            !delivered_receipts_verified(&store, &snapshot)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn shadow_blocks_but_other_legacy_receipts_are_historical() {
        let store = crate::store::Store::new(Arc::new(MemoryStore::new()));
        let shadow = snapshot_with(OutboxState::Delivered {
            receipt: OutboxReceipt::legacy("shadow"),
        });
        assert!(!delivered_receipts_verified(&store, &shadow).await.unwrap());
        let legacy = snapshot_with(OutboxState::Delivered {
            receipt: OutboxReceipt::legacy("message"),
        });
        assert!(delivered_receipts_verified(&store, &legacy).await.unwrap());
    }
}
