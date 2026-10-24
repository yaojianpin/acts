//! Recovery: the exceptional work the normal path cannot finish.
//!
//! Expired claims, commits to finalize, effect-unknown reconciliation and
//! missing ready records — the scanner walks them by bounded passes with an
//! in-memory cursor, and each disposition runs inside the same command lane
//! and revision fences as every other writer.

use super::*;

/// The executor's proof about one `EffectUnknown` operation (A1c): the fact
/// Core's recovery decision reads, plus the payload only the shell can hand to
/// the normal effect planner when the decision says `ReplayEffect`.
pub(crate) struct ObservedEffect {
    receipt: EffectReceipt,
    observation: Option<effects::EffectObservation>,
}

impl KernelHost {
    /// Ask the executor for its proof about one `EffectUnknown` operation
    /// (A1c).
    ///
    /// This is the observation half of recovery: it performs the executor IO
    /// by the operation's stable effect key and converts the answer into the
    /// fact Core's `RecoverOperation` reads. It never decides what the answer
    /// means — replay, retry and manual are [`recover_operation`]'s call.
    pub(in crate::scheduler::shell) async fn observe_effect(
        &self,
        observed: &Observed,
    ) -> Result<Option<ObservedEffect>> {
        if observed.kind != OperationKind::ApplyAction
            || !matches!(
                observed.persisted_phase,
                OperationState::EffectUnknown { .. }
            )
        {
            return Ok(None);
        }
        let Some(loaded) = self.load(&observed.operation_id.process_id).await? else {
            return Ok(None);
        };
        let Some(task) = loaded.snapshot.tasks.get(&observed.operation_id.task) else {
            return Ok(None);
        };
        let model_node = task.model_node.clone();
        let Some((runner, request)) = self.step_effect(
            &loaded.snapshot,
            Some(&loaded.snapshot),
            &observed.operation_id.task,
            &model_node,
        )?
        else {
            return Ok(None);
        };
        let key = EffectKey::of(&observed.operation_id);
        let observation = match runner.observe(&key).await {
            Ok(observation) => observation,
            Err(error) => {
                tracing::warn!(
                    operation = ?observed.operation_id,
                    error = %error,
                    "querying the effect observation failed; manual reconciliation still owns the operation"
                );
                return Ok(Some(ObservedEffect {
                    receipt: EffectReceipt::Unknown,
                    observation: None,
                }));
            }
        };
        let receipt = match &observation {
            effects::EffectObservation::Applied(_) => EffectReceipt::Applied,
            effects::EffectObservation::Failed(_) => EffectReceipt::Failed,
            // Re-running can only be safe when the effect itself is provably
            // idempotent; the executor keeps the same key, so a duplicate
            // delivery is deduplicated on its side.
            effects::EffectObservation::NotApplied => EffectReceipt::NotApplied {
                idempotent: runner.idempotent(&request),
            },
            effects::EffectObservation::Unknown => EffectReceipt::Unknown,
        };
        let observation = matches!(receipt, EffectReceipt::Applied | EffectReceipt::Failed)
            .then_some(observation);
        Ok(Some(ObservedEffect {
            receipt,
            observation,
        }))
    }

    /// The unknown effects of one run, in operation order (A1c).
    pub(crate) async fn effect_unknowns(
        &self,
        process_id: &str,
    ) -> Result<Vec<crate::engine::EffectUnknown>> {
        let Some(snapshot) = self.snapshot(process_id).await? else {
            return Ok(Vec::new());
        };
        let mut unknowns = snapshot
            .operations
            .values()
            .filter_map(|record| match record.state {
                OperationState::EffectUnknown { attempt } => Some(crate::engine::EffectUnknown {
                    pid: process_id.to_string(),
                    tid: record.id.task.wire(process_id),
                    sequence: record.id.sequence,
                    key: EffectKey::of(&record.id).wire(),
                    attempt,
                }),
                _ => None,
            })
            .collect::<Vec<_>>();
        unknowns.sort_by(|left, right| {
            left.sequence
                .cmp(&right.sequence)
                .then_with(|| left.tid.cmp(&right.tid))
        });
        Ok(unknowns)
    }

    /// Resolve one unknown effect by its stable key (A1c).
    pub(crate) async fn resolve_effect_by_key(
        &self,
        process_id: &str,
        key: &str,
        resolution: crate::engine::EffectResolution,
    ) -> Result<()> {
        let snapshot = self
            .snapshot(process_id)
            .await?
            .ok_or_else(|| ActError::Runtime(format!("aggregate {process_id} not found")))?;
        let operation = snapshot
            .operations
            .values()
            .find(|record| EffectKey::of(&record.id).wire() == key)
            .map(|record| record.id.clone())
            .ok_or_else(|| {
                ActError::Runtime(format!(
                    "no operation with effect key {key:?} in run {process_id}"
                ))
            })?;
        self.resolve_effect(&operation, resolution).await
    }

    /// The operator's durable decision for one `EffectUnknown` operation
    /// (A1c): replay it as applied (with the outputs the operator captured),
    /// or prove it never ran so it may be retried.
    pub(crate) async fn resolve_effect(
        &self,
        id: &OperationId,
        resolution: crate::engine::EffectResolution,
    ) -> Result<()> {
        // A8: a human decision is control-plane work with its own budget. It
        // never asks for an execution slot, so a full run quota cannot park it
        // and it cannot park a new run.
        let _reconciliation = self
            .enter_admission(AdmissionCategory::Reconciliation)
            .await;
        let Some(loaded) = self.load(&id.process_id).await? else {
            return Err(ActError::Runtime(format!(
                "aggregate {} not found",
                id.process_id
            )));
        };
        let record = loaded
            .snapshot
            .operations
            .get(id)
            .cloned()
            .ok_or_else(|| ActError::Runtime(format!("operation {id:?} not found")))?;
        if !matches!(record.state, OperationState::EffectUnknown { .. }) {
            return Err(ActError::Runtime(format!(
                "operation {id:?} is {:?}, not an unknown effect",
                record.state
            )));
        }
        match resolution {
            crate::engine::EffectResolution::Applied { outputs } => {
                let applied = self
                    .converge_observed_effect(
                        id,
                        effects::EffectObservation::Applied(effects::EffectOutcome::from_outputs(
                            outputs,
                        )),
                    )
                    .await?;
                if !applied {
                    return Err(ActError::Runtime(format!(
                        "operation {id:?} is no longer reconcilable"
                    )));
                }
                Ok(())
            }
            crate::engine::EffectResolution::NotApplied => {
                if !self.retry_unknown_effect(&record).await? {
                    return Err(ActError::Runtime(format!(
                        "operation {id:?} is no longer reconcilable"
                    )));
                }
                Ok(())
            }
        }
    }

    /// One driver tick for the test suites. The production driver runs the
    /// same four passes on independent intervals (A10); this helper runs them
    /// once, in order, so a test that calls it observes every loop.
    #[cfg(test)]
    pub(crate) async fn tick(&self, now_ms: i64) -> Result<RecoveryReport> {
        self.drive_ready(now_ms, PAGE_SIZE).await?;
        let report = self.recover(now_ms).await?;
        self.repair_projections().await;
        Ok(report)
    }

    /// Re-derive exceptional work from the recovery index.
    ///
    /// This method is only the IO shell around
    /// [`recover_operation`](crate::scheduler::core::recovery::planner::recover_operation)
    /// (A7): the paging walk, one [`Observed`] per row, and the directive the
    /// pure decision names. Normal `Pending`/`RetryScheduled` operations are
    /// not woken here: they own a durable ready record and the driver's due
    /// pass is their liveness source. Recovery handles what that path cannot:
    /// an expired claim is reclaimed, a committed-but-unfinalized operation is
    /// completed through the normal post-commit program, effect-unknown and
    /// quarantined operations follow the executor's proof or go to a human,
    /// and a missing ready record (an older database, a damaged index) is
    /// repaired.
    #[cfg(test)]
    pub(crate) async fn recover(&self, now_ms: i64) -> Result<RecoveryReport> {
        self.recover_with_budget(now_ms, PAGE_SIZE).await
    }

    /// The recovery scanner's own pass (A10). The scan resumes at the last
    /// examined index entry when `budget` is reached, so a large exceptional
    /// backlog is bounded work per round; an in-memory cursor is only a
    /// scheduling hint and restarts from the beginning after a restart.
    ///
    /// Projection repair is **not** part of this pass: it has its own loop
    /// ([`Self::repair_projections_batch`]) and its own interval, so changing
    /// the recovery cadence cannot change projection lag and vice versa.
    pub(crate) async fn recover_with_budget(
        &self,
        now_ms: i64,
        budget: usize,
    ) -> Result<RecoveryReport> {
        self.ensure_boot(now_ms).await?;
        let budget = budget.max(1);
        let _recovery = self.enter_admission(AdmissionCategory::Recovery).await;
        let mut report = RecoveryReport::default();
        let mut cursor = self.recovery_cursor.lock().clone();
        let mut more_pending = false;
        loop {
            let page = self
                .recovery
                .recoverable_page(cursor.as_ref(), PAGE_SIZE.min(budget))
                .await
                .map_err(runtime)?;
            if page.operations.is_empty() && page.next.is_none() {
                cursor = None;
                break;
            }
            for candidate in page.operations {
                if report.scanned >= budget {
                    more_pending = true;
                    break;
                }
                let id = candidate.operation.id.clone();
                let process_key = id.process_id.clone();
                report.scanned += 1;
                self.recover_observed(candidate, now_ms, &mut report).await;
                cursor = Some(RecoveryCursor {
                    process_key,
                    operation: Some(id),
                });
            }
            if more_pending {
                break;
            }
            match page.next {
                Some(next) => {
                    if report.scanned >= budget {
                        cursor = Some(next);
                        more_pending = true;
                        break;
                    }
                    cursor = Some(next);
                }
                None => {
                    cursor = None;
                    break;
                }
            }
        }
        *self.recovery_cursor.lock() = if more_pending { cursor } else { None };
        {
            let mut metrics = self.recovery_metrics.lock();
            metrics.scanned = report.scanned;
            metrics.more_pending = more_pending;
            metrics.passes += 1;
            metrics.reclaimed += report.reclaimed.len() as u64;
            metrics.ready_repaired += report.ready_repaired.len() as u64;
            metrics.finalized += report.finalized.len() as u64;
            metrics.effect_applied += report.effect_applied.len() as u64;
            metrics.effect_retried += report.effect_retried.len() as u64;
            metrics.reconciliation_required = report.reconciliation_required.len();
        }
        Ok(report)
    }

    /// The one-time boot passes: rebuild the derived indexes and catch the
    /// read model up from the aggregates. Both are idempotent and independent
    /// of the periodic loop that notices they are still pending (A10).
    pub(crate) async fn ensure_boot(&self, now_ms: i64) -> Result<()> {
        self.ensure_ready_boot(now_ms).await?;
        self.ensure_projection_boot().await;
        Ok(())
    }

    /// Rebuild the derived ready/recovery indexes from the authoritative
    /// aggregates once per host (A2's cold-start repair). Owned by the
    /// ready/timer loop: a recovery cadence cannot delay it and its failure is
    /// retried by the next ready pass.
    pub(crate) async fn ensure_ready_boot(&self, now_ms: i64) -> Result<()> {
        if self
            .boot_reconciled
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Ok(());
        }
        let repaired = self
            .processes
            .repair_all_ready(now_ms, self.timer_poll_ms)
            .await
            .map_err(runtime)?;
        self.boot_reconciled
            .store(true, std::sync::atomic::Ordering::SeqCst);
        if repaired > 0 {
            self.ready_metrics.lock().repaired += repaired as u64;
            self.ready.notify_one();
        }
        Ok(())
    }

    /// One observed operation, from the observation to the executed decision.
    ///
    /// This is the IO half of [`recover_operation`]. It collects the
    /// executor's proof when the decision asks for one, commits the command
    /// Core names under the pid's command lane (the same lane and the same
    /// revision fence a normal operation uses), and runs the directive. No
    /// branch here decides what a phase means: `ReclaimReady` versus
    /// `ReclaimUnknown`, replay versus retry, and finalize are Core's answers.
    ///
    /// A command that loses its guard is left to the next scan — the state it
    /// was formed from moved on, so the new state will be observed afresh.
    async fn recover_observed(
        &self,
        candidate: RecoveryCandidate,
        now_ms: i64,
        report: &mut RecoveryReport,
    ) {
        let mut observed = Observed::from_record(&candidate.operation, candidate.version, now_ms);
        // The payload of an `Applied`/`Failed` observation, kept for the
        // `ReplayEffect` directive. Core sees only the receipt.
        let mut effect = None;
        loop {
            let decision = recover_operation(&observed);
            match decision.directive {
                RecoveryDirective::AwaitOwner => {
                    report.awaiting_owner.push(observed.operation_id);
                    return;
                }
                RecoveryDirective::IgnoreTerminal => return,
                RecoveryDirective::RepairReady => {
                    match self
                        .processes
                        .repair_ready(
                            &observed.operation_id.process_id,
                            now_ms,
                            self.timer_poll_ms,
                        )
                        .await
                    {
                        Ok(true) => {
                            self.ready_metrics.lock().repaired += 1;
                            report.ready_repaired.push(observed.operation_id);
                            self.ready.notify_one();
                        }
                        Ok(false) => {}
                        Err(error) => {
                            tracing::debug!(
                                operation = ?observed.operation_id,
                                error = %error,
                                "repairing the ready record failed; next scan retries"
                            );
                        }
                    }
                    return;
                }
                RecoveryDirective::ReclaimReady => {
                    let Some(command) = decision.command else {
                        tracing::debug!(
                            operation = ?observed.operation_id,
                            "reclaim decision carried no command"
                        );
                        return;
                    };
                    // A4: the reclaim is a command of the same pid — it takes
                    // the lane for the event alone. The revision guard the
                    // event carries stays the fence against a claim that moved
                    // since the scan.
                    let reclaimed = {
                        let _lane = self
                            .lock_command_lane(&observed.operation_id.process_id)
                            .await;
                        self.operation_event(
                            &observed.operation_id,
                            command.expected_revision,
                            command.event,
                        )
                        .await
                    };
                    match reclaimed {
                        Ok((reclaimed, _)) => {
                            report.reclaimed.push(reclaimed.id.clone());
                            // The event's own commit wrote the ready record;
                            // the wake is that record's accelerator, not a
                            // second business decision.
                            self.wake(&reclaimed.id);
                        }
                        Err(error) => {
                            tracing::debug!(
                                operation = ?observed.operation_id,
                                error = %error,
                                "reclaim lost; next scan retries"
                            );
                        }
                    }
                    return;
                }
                RecoveryDirective::ReclaimUnknown => {
                    let Some(command) = decision.command else {
                        tracing::debug!(
                            operation = ?observed.operation_id,
                            "reclaim decision carried no command"
                        );
                        return;
                    };
                    let reclaimed = {
                        let _lane = self
                            .lock_command_lane(&observed.operation_id.process_id)
                            .await;
                        self.operation_event(
                            &observed.operation_id,
                            command.expected_revision,
                            command.event,
                        )
                        .await
                    };
                    match reclaimed {
                        Ok((reclaimed, loaded)) => {
                            // The commit turned the operation `EffectUnknown`.
                            // Observe the new state and let Core decide again;
                            // the effect itself is never re-driven here.
                            observed = Observed::from_record(
                                &reclaimed,
                                loaded.snapshot.version.clone(),
                                now_ms,
                            );
                        }
                        Err(error) => {
                            tracing::debug!(
                                operation = ?observed.operation_id,
                                error = %error,
                                "reclaim lost; next scan retries"
                            );
                            return;
                        }
                    }
                }
                RecoveryDirective::FinalizeCommit => {
                    let Some(command) = decision.command else {
                        tracing::debug!(
                            operation = ?observed.operation_id,
                            "finalize decision carried no command"
                        );
                        return;
                    };
                    // A4: the close is a command of the same pid, fenced by
                    // the operation revision. The lane is dropped before the
                    // post-commit program, which may report lifecycle and
                    // deliver records.
                    let finalized = {
                        let _lane = self
                            .lock_command_lane(&observed.operation_id.process_id)
                            .await;
                        self.operation_event(
                            &observed.operation_id,
                            command.expected_revision,
                            command.event,
                        )
                        .await
                    };
                    match finalized {
                        Ok((finalized, mut loaded)) => {
                            // A6/A7: the same post-commit program a normal
                            // `dispatch` runs on the snapshot its commit
                            // returned — rows current, then lifecycle, outbox
                            // (nothing new here), then admission.
                            let admission = self.admissions.lock().directive(&loaded.snapshot);
                            let decision = Decision::recovered(&loaded.snapshot, admission);
                            if let Err(error) = self
                                .execute_decision(
                                    &mut loaded,
                                    &decision,
                                    operation::Projection::Touched,
                                    &[],
                                )
                                .await
                            {
                                tracing::warn!(
                                    operation = ?finalized.id,
                                    error = %error,
                                    "executing a finalize commit's decision failed; the recovery pass retries the rest"
                                );
                            }
                            report.finalized.push(finalized.id);
                        }
                        Err(error) => {
                            tracing::debug!(
                                operation = ?observed.operation_id,
                                error = %error,
                                "finalize lost; next scan retries"
                            );
                        }
                    }
                    return;
                }
                RecoveryDirective::ObserveEffect => match self.observe_effect(&observed).await {
                    Ok(Some(proof)) => {
                        effect = proof.observation;
                        observed = observed.with_effect_receipt(proof.receipt);
                    }
                    Ok(None) => {
                        report.reconciliation_required.push(observed.operation_id);
                        return;
                    }
                    Err(error) => {
                        tracing::debug!(
                            operation = ?observed.operation_id,
                            error = %error,
                            "effect observation failed; next scan retries"
                        );
                        report.reconciliation_required.push(observed.operation_id);
                        return;
                    }
                },
                RecoveryDirective::ReplayEffect => {
                    let Some(observation) = effect.take() else {
                        report.reconciliation_required.push(observed.operation_id);
                        return;
                    };
                    match self
                        .converge_observed_effect(&observed.operation_id, observation)
                        .await
                    {
                        Ok(true) => report.effect_applied.push(observed.operation_id),
                        Ok(false) => report.reconciliation_required.push(observed.operation_id),
                        Err(error) => {
                            tracing::warn!(
                                operation = ?observed.operation_id,
                                error = %error,
                                "replaying an observed effect failed; the operation stays unknown"
                            );
                            report.reconciliation_required.push(observed.operation_id);
                        }
                    }
                    return;
                }
                RecoveryDirective::RetryEffect => {
                    match self.retry_unknown_effect(&observed.record()).await {
                        Ok(true) => report.effect_retried.push(observed.operation_id),
                        Ok(false) => report.reconciliation_required.push(observed.operation_id),
                        Err(error) => {
                            tracing::warn!(
                                operation = ?observed.operation_id,
                                error = %error,
                                "re-queuing an idempotent effect failed; the next scan retries"
                            );
                            report.reconciliation_required.push(observed.operation_id);
                        }
                    }
                    return;
                }
                RecoveryDirective::Manual => {
                    report.reconciliation_required.push(observed.operation_id);
                    return;
                }
            }
        }
    }
}
