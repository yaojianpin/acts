//! Outbox delivery: the fence, the claim state machine and the retry pass.
//!
//! A record is delivered at-least-once: the normal path applies its claim and
//! the transport's outcome in one in-memory sequence before the one batch
//! commit; the retry pass persists the claim first, so a concurrent worker
//! cannot deliver the same record under the same fence.

use super::*;

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

/// The default outbox transport: it logs each record and confirms it — what a
/// host with nothing attached (the kernel's own tests, and any embedder that
/// only wants the durable aggregate) does. The serving host replaces it with
/// [`message::MessageForwarder`] so records reach the engine channel.
pub(crate) struct ShadowTransport;

impl crate::scheduler::core::outbox::outbox_dispatch::OutboxTransport for ShadowTransport {
    fn deliver(
        &mut self,
        record: &crate::scheduler::core::outbox::records::OutboxRecord,
    ) -> crate::scheduler::core::outbox::outbox_dispatch::DeliveryOutcome {
        tracing::debug!(
            pid = %record.id.process_id,
            key = %record.id.key,
            topic = %record.topic,
            "kernel outbox delivered (shadow)"
        );
        crate::scheduler::core::outbox::outbox_dispatch::DeliveryOutcome::Delivered {
            receipt: "shadow".into(),
        }
    }
}

impl KernelHost {
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
        let process_id = loaded.snapshot.process_id.clone();
        let snapshot = &loaded.snapshot;
        let now = SystemClock.now_ms();
        let mut confirmations = Vec::with_capacity(ids.len());
        {
            let mut transport = self.transport.write();
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
                let outcome = transport.deliver(record);
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
            let outcome = self.transport.write().deliver(&claimed);
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
        let now = SystemClock.now_ms();
        let budget = budget.max(1);
        let mut delivered = 0usize;
        let mut processed = 0usize;
        let mut deferred_records = 0u64;
        let mut settled: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
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
            let remaining = budget.saturating_sub(processed).max(1);
            let page = self
                .processes
                .dispatchable_outbox_page(now, cursor.as_ref(), PAGE_SIZE.min(remaining))
                .await
                .map_err(runtime)?;
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
        }
        // The deliveries that settled decide the read model's `removable`
        // flag: a run that finished before its records were delivered only
        // hands its rows (and directory) to the sweeper once they are.
        for process_id in &settled {
            self.refresh_proc_row(process_id).await;
        }
        Ok(delivered)
    }
}
