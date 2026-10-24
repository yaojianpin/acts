//! Admission: the slot policy of the five quota categories (A8).
//!
//! Execution bounds the runs in flight; the other four categories bound their
//! own work and never consume another category's budget. The rule itself is
//! [`concurrency`](crate::scheduler::core::plan::concurrency)'s; this module
//! is only the in-memory half the shell owns.

use super::*;

/// The configured bounds of the five admission categories (A8). Execution
/// bounds normal runs in flight; the other four bound their own work and never
/// consume another category's budget.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AdmissionCaps {
    pub(crate) execution: usize,
    pub(crate) effect: usize,
    pub(crate) recovery: usize,
    pub(crate) reconciliation: usize,
    pub(crate) projection: usize,
}

impl AdmissionCaps {
    /// The caps a host without an explicit admission configuration uses: the
    /// execution bound it was given and the process defaults for the rest.
    #[cfg(test)]
    pub(crate) fn new(execution: usize) -> Self {
        Self {
            execution,
            effect: crate::config::DEFAULT_EFFECT_CONCURRENCY,
            recovery: crate::config::DEFAULT_CONTROL_CONCURRENCY,
            reconciliation: crate::config::DEFAULT_CONTROL_CONCURRENCY,
            projection: crate::config::DEFAULT_CONTROL_CONCURRENCY,
        }
    }

    fn from_config(config: &crate::Config) -> Self {
        Self {
            execution: config.cache_cap() as usize,
            effect: config.effect_concurrency(),
            recovery: config.recovery_concurrency(),
            reconciliation: config.reconciliation_concurrency(),
            projection: config.projection_concurrency(),
        }
    }
}

/// One auxiliary quota's counter and waiters. The rule is
/// [`decision::quota`]; this type is only the in-memory half the shell owns.
#[derive(Debug, Default)]
pub(crate) struct AdmissionGate {
    cap: usize,
    active: std::sync::atomic::AtomicUsize,
    admitted: std::sync::atomic::AtomicU64,
    waiters: tokio::sync::Notify,
}

impl AdmissionGate {
    pub(in crate::scheduler::shell) fn new(cap: usize) -> Arc<Self> {
        Arc::new(Self {
            cap,
            active: std::sync::atomic::AtomicUsize::new(0),
            admitted: std::sync::atomic::AtomicU64::new(0),
            waiters: tokio::sync::Notify::new(),
        })
    }

    /// Take one permit, waiting while the category is saturated. The waiter
    /// is registered before the counter is read, so a permit released in
    /// between is never lost.
    pub(in crate::scheduler::shell) async fn enter(self: &Arc<Self>) -> AdmissionLease {
        use std::sync::atomic::Ordering::SeqCst;
        loop {
            let notified = self.waiters.notified();
            let active = self.active.load(SeqCst);
            if decision::quota(self.cap, active) == QuotaDirective::Wait {
                notified.await;
                continue;
            }
            if self
                .active
                .compare_exchange(active, active.saturating_add(1), SeqCst, SeqCst)
                .is_ok()
            {
                self.admitted.fetch_add(1, SeqCst);
                return AdmissionLease { gate: self.clone() };
            }
            // Another entrant took the permit between the read and the CAS.
            // Re-check immediately: with a cap above one a free permit may
            // still exist, and waiting here could strand it.
        }
    }

    pub(in crate::scheduler::shell) fn metrics(&self) -> QuotaMetrics {
        QuotaMetrics {
            cap: self.cap,
            active: self.active.load(std::sync::atomic::Ordering::SeqCst),
            admitted: self.admitted.load(std::sync::atomic::Ordering::SeqCst),
        }
    }
}

/// One held auxiliary permit; dropping it wakes one waiter.
pub(crate) struct AdmissionLease {
    gate: Arc<AdmissionGate>,
}

impl Drop for AdmissionLease {
    fn drop(&mut self) {
        self.gate
            .active
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        self.gate.waiters.notify_one();
    }
}

/// One auxiliary quota's gauge: the configured cap, holders now, and how many
/// holders it admitted over the host's lifetime.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct QuotaMetrics {
    pub(crate) cap: usize,
    pub(crate) active: usize,
    pub(crate) admitted: u64,
}

/// Every category's gauge (A8). Execution reports its slot set and parked
/// queue; each auxiliary quota reports its own counter.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct AdmissionMetrics {
    pub(crate) execution: QuotaMetrics,
    pub(crate) parked: usize,
    pub(crate) effect: QuotaMetrics,
    pub(crate) recovery: QuotaMetrics,
    pub(crate) reconciliation: QuotaMetrics,
    pub(crate) projection: QuotaMetrics,
    /// Runs the last decisions observed waiting on manual reconciliation.
    pub(crate) quarantined_runs: usize,
}

/// The driver's slot bookkeeping: which runs hold one of the execution slots,
/// which wait for one, and the four auxiliary quotas (A8).
///
/// The configured number used to bound the legacy engine's *resident object
/// set*; the kernel holds no resident objects, so the same number is redefined
/// as the bound on runs that are in flight at once (see
/// [`concurrency`](crate::scheduler::core::plan::concurrency) for the rule).
/// A run that cannot take a slot *parks*: a start seeds it with its root still
/// `Created` — the read model's `None` process state, exactly the legacy parked
/// row — and leaves that root's operation undriven, while a run that was
/// already in flight keeps the row it had (the legacy boot-resume overflow's
/// shape). Either way the run takes no operation, and the oldest parked run
/// (`__created` ascending, the legacy row's `timestamp` order) is what a freed
/// slot admits.
///
/// The execution slot is *not* a lease on a run forever: a run that settles,
/// or one whose only remaining route is manual reconciliation, releases it in
/// the decision that observed that fact. Effects, recovery, human
/// reconciliation and projections have their own gates, so a saturated
/// category never holds another category's work.
///
/// The legacy resident set answered the same question through `Cache`; here
/// the only writers are [`KernelHost::start_process`] (which reserves) and the
/// driver's operations (which settle), so this set is the single answer to "may
/// this run operation". Like that set it is in-memory: a restart starts it
/// empty, and the runs still in flight take their slots again through
/// [`KernelHost::place_run`] when the ready queue hands them their first turn.
/// The first `cap` of them are admitted in the order the ready index returns
/// them rather than by `__created` (the store keeps no boot index to sort by),
/// while every later admission is by stamp, exactly like the legacy parked
/// queue.
#[derive(Default)]
pub(crate) struct Admissions {
    /// The bound on runs in flight at once.
    cap: usize,
    /// Runs holding an execution slot: admitted here and not yet settled or
    /// quarantined.
    active: std::collections::HashSet<String>,
    /// Execution slots granted over the host's lifetime (a metric).
    admitted: u64,
    /// Parked runs, oldest first, with the operation that admits each: the
    /// seeded root `ExecuteTask` whose operation starts the parked root. The key is
    /// the run's `__created` stamp; two runs seeded in the same millisecond
    /// are ordered by pid, so no run can hide behind an equal key.
    parked: std::collections::BTreeMap<(i64, String), OperationId>,
    /// Concurrent external effects.
    effect: Arc<AdmissionGate>,
    /// Recovery scans and their exceptional commands.
    recovery: Arc<AdmissionGate>,
    /// Human reconciliation of `EffectUnknown`/`Quarantined` operations.
    reconciliation: Arc<AdmissionGate>,
    /// Row projections and their repair work.
    projection: Arc<AdmissionGate>,
    /// Runs whose remaining route is manual reconciliation, as the last
    /// decision observed them. A gauge for operators; the admission rule reads
    /// the aggregate, never this set.
    quarantined: std::collections::HashSet<String>,
}

impl Admissions {
    pub(crate) fn new(caps: AdmissionCaps) -> Self {
        Self {
            cap: caps.execution,
            effect: AdmissionGate::new(caps.effect),
            recovery: AdmissionGate::new(caps.recovery),
            reconciliation: AdmissionGate::new(caps.reconciliation),
            projection: AdmissionGate::new(caps.projection),
            ..Self::default()
        }
    }

    /// The gate for an auxiliary category. Execution has no auxiliary gate:
    /// its set is the ledger itself and its rule is [`decision::place`].
    pub(in crate::scheduler::shell) fn gate(
        &self,
        category: AdmissionCategory,
    ) -> Option<Arc<AdmissionGate>> {
        match category {
            AdmissionCategory::Execution => None,
            AdmissionCategory::Effect => Some(self.effect.clone()),
            AdmissionCategory::Recovery => Some(self.recovery.clone()),
            AdmissionCategory::Reconciliation => Some(self.reconciliation.clone()),
            AdmissionCategory::Projection => Some(self.projection.clone()),
        }
    }

    pub(in crate::scheduler::shell) fn metrics(&self) -> AdmissionMetrics {
        AdmissionMetrics {
            execution: QuotaMetrics {
                cap: self.cap,
                active: self.active.len(),
                admitted: self.admitted,
            },
            parked: self.parked.len(),
            effect: self.effect.metrics(),
            recovery: self.recovery.metrics(),
            reconciliation: self.reconciliation.metrics(),
            projection: self.projection.metrics(),
            quarantined_runs: self.quarantined.len(),
        }
    }

    /// Record the last decision's view of a run's reconciliation hold. The
    /// admission rule itself reads the aggregate; this set is the operator
    /// gauge.
    pub(in crate::scheduler::shell) fn note_quarantined(
        &mut self,
        process_id: &str,
        quarantined: bool,
    ) {
        if quarantined {
            self.quarantined.insert(process_id.to_string());
        } else {
            self.quarantined.remove(process_id);
        }
    }

    /// The pure rule against the set it bounds.
    fn decide(&self, state: crate::scheduler::core::model::task_graph::TaskState) -> Admission {
        infallible(Admit::plan((), &(self.cap, self.active.len(), state)))
    }

    /// Reserve a slot for a fresh run — its root is seeded `Ready`, which is
    /// the state the rule is asked about — or decide that it parks. The
    /// reservation and the decision are one step, so concurrent starts cannot
    /// both take the last slot.
    pub(in crate::scheduler::shell) fn reserve(&mut self, process_id: &str) -> Admission {
        let admission = self.decide(crate::scheduler::core::model::task_graph::TaskState::Ready);
        if admission == Admission::Admit {
            self.activate(process_id);
        }
        admission
    }

    pub(in crate::scheduler::shell) fn is_parked(&self, process_id: &str) -> bool {
        self.parked.keys().any(|(_, parked)| parked == process_id)
    }

    /// Whether the run holds a slot.
    pub(in crate::scheduler::shell) fn holds(&self, process_id: &str) -> bool {
        self.active.contains(process_id)
    }

    pub(in crate::scheduler::shell) fn activate(&mut self, process_id: &str) {
        if self.active.insert(process_id.to_string()) {
            self.admitted = self.admitted.saturating_add(1);
        }
    }

    pub(in crate::scheduler::shell) fn park(
        &mut self,
        created_ms: i64,
        process_id: &str,
        admission: &OperationId,
    ) {
        self.parked
            .insert((created_ms, process_id.to_string()), admission.clone());
    }

    /// Forget a reservation whose run never became durable (its creation
    /// failed): the slot must not stay taken — and the pid must not stay
    /// parked — by a run that does not exist.
    pub(in crate::scheduler::shell) fn abandon(&mut self, process_id: &str) {
        self.active.remove(process_id);
        self.parked.retain(|(_, parked), _| parked != process_id);
        self.quarantined.remove(process_id);
    }

    /// Take a free slot for a parked run — the check a start makes after it
    /// parked, because a slot can free while its aggregate is being written
    /// and nothing else would come back for that run.
    pub(in crate::scheduler::shell) fn take_free_slot(
        &mut self,
        created_ms: i64,
        process_id: &str,
    ) -> bool {
        if self.active.len() >= self.cap
            || self
                .parked
                .remove(&(created_ms, process_id.to_string()))
                .is_none()
        {
            return false;
        }
        self.activate(process_id);
        true
    }

    /// A run that no longer needs an execution slot frees it; returns the
    /// operation that admits the parked run taking it, if there is one. The
    /// caller wakes that operation immediately — the tick is only a fallback.
    pub(in crate::scheduler::shell) fn release(&mut self, process_id: &str) -> Option<OperationId> {
        if !self.active.remove(process_id) {
            return None;
        }
        if self.active.len() >= self.cap {
            return None;
        }
        let ((_, process_id), admission) = self.parked.pop_first()?;
        self.active.insert(process_id);
        Some(admission)
    }

    /// The core admission policy over this in-memory set (A6/A8): the shell
    /// owns the set, [`crate::scheduler::core::decision::place`] owns the rule.
    pub(in crate::scheduler::shell) fn directive(
        &self,
        snapshot: &ProcessSnapshot,
    ) -> AdmissionDirective {
        let standing = if self.is_parked(&snapshot.process_id) {
            SlotStanding::Parked
        } else if self.holds(&snapshot.process_id) {
            SlotStanding::Holding
        } else {
            SlotStanding::Unplaced
        };
        decision::place(
            SlotObservation {
                standing,
                cap: self.cap,
                active: self.active.len(),
            },
            RunFacts::of(snapshot),
        )
    }

    /// Execute one admission directive against the set. Returns the operation
    /// that admits the parked run taking a freed slot, if any.
    pub(in crate::scheduler::shell) fn apply(
        &mut self,
        snapshot: &ProcessSnapshot,
        directive: AdmissionDirective,
    ) -> Option<OperationId> {
        match directive {
            AdmissionDirective::Park => {
                if !self.is_parked(&snapshot.process_id) {
                    self.park(
                        created_stamp(snapshot),
                        &snapshot.process_id,
                        &decision::resume_operation(snapshot),
                    );
                }
                None
            }
            AdmissionDirective::Drive(SlotDirective::Release) => self.release(&snapshot.process_id),
            AdmissionDirective::Drive(SlotDirective::Take) => {
                self.activate(&snapshot.process_id);
                None
            }
            AdmissionDirective::Drive(SlotDirective::Keep | SlotDirective::NoSlot) => None,
        }
    }
}

impl KernelHost {
    /// The caps a [`Config`](crate::Config) names (A8). Public entry points
    /// that build a host without one use [`AdmissionCaps::new`]'s defaults.
    pub(crate) fn caps_from_config(config: &crate::Config) -> AdmissionCaps {
        AdmissionCaps::from_config(config)
    }

    /// Take one permit of an auxiliary admission category. Execution has no
    /// auxiliary gate — its set is the ledger and [`Self::place_run`] is its
    /// admission path.
    pub(in crate::scheduler::shell) async fn enter_admission(
        &self,
        category: AdmissionCategory,
    ) -> AdmissionLease {
        assert!(
            category != AdmissionCategory::Execution,
            "execution admission is the slot ledger, not a gate"
        );
        let gate = self
            .admissions
            .lock()
            .gate(category)
            .expect("execution admission is the slot ledger, not a gate");
        gate.enter().await
    }

    /// Every admission category's gauge (A8).
    pub(crate) fn admission_metrics(&self) -> AdmissionMetrics {
        self.admissions.lock().metrics()
    }

    /// The slot bookkeeping for one run, from a fresh snapshot of it. Returns
    /// whether the operation that asked may run.
    ///
    /// - a run this driver admitted and whose executions all settled frees its
    ///   slot, and the parked run that takes it is returned for admission;
    /// - a parked run keeps waiting: its operations stay pending and undriven
    ///   until a slot frees;
    /// - a run this driver never admitted — one already in flight when it
    ///   booted, since every start reserves through [`Self::start_process`] —
    ///   takes a slot here, or parks exactly like a start that found every
    ///   slot taken. This is the boot pass: the ready queue hands every
    ///   unfinished run its first turn, and this is where the cap meets it.
    pub(in crate::scheduler::shell) fn place_run(
        &self,
        snapshot: &crate::scheduler::core::commit::aggregate::ProcessSnapshot,
    ) -> (bool, Option<OperationId>) {
        let mut admissions = self.admissions.lock();
        // The rule is core's (`decision::place`); this set is only the state
        // it reads and the mutations it names.
        let directive = admissions.directive(snapshot);
        let freed = admissions.apply(snapshot, directive);
        (directive.drives(), freed)
    }

    /// Make a parked run's ready record due now, then wake it. Used when a
    /// freed slot admits it: its record was deferred while it waited, and the
    /// wake is only the accelerator over that durable record.
    pub(in crate::scheduler::shell) async fn admit_run(
        &self,
        operation: &OperationId,
    ) -> Result<()> {
        self.processes
            .set_ready_at(operation, SystemClock.now_ms())
            .await
            .map_err(runtime)?;
        self.wake(operation);
        Ok(())
    }
}
