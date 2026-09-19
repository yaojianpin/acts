//! Ownership of the durable outbox operations a lane job is running.
//!
//! A propagation operation (`next`/`error`/`abort`) owns a durable outbox row
//! for its whole effect: it is created before the operation is dispatched and
//! closed only after the effect is durable. Between the two, the row alone
//! cannot say whether a job is still working it — a job that ends without
//! closing its record (an early return, a panic, a cancelled future, a close
//! whose write was lost) leaves exactly the row a crash does, and the record
//! is then stranded: nothing re-drives it until the next engine start, and the
//! task it belongs to waits behind its open phase forever while its process
//! holds a resident slot.
//!
//! This registry is that missing ownership: the job claims the operation
//! through a [`OpClaim`] guard (installed around the effect, released on
//! *every* exit path, unwinding included) and the periodic outbox pass
//! ([`crate::scheduler::Runtime::recover_outbox`]) re-drives what no job owns
//! any more. The durable row stays the source of truth for *what* to do — the
//! claim only says whether doing it again would race a live job — so a record
//! left behind by a dead engine is still recovered by the boot replay.

use crate::{data::OpType, scheduler::Task};
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

/// Identity of one durable operation: the task that owns it and its type.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct OpKey {
    pub pid: String,
    pub tid: String,
    pub r#type: OpType,
}

impl OpKey {
    pub(crate) fn new(pid: &str, tid: &str, r#type: OpType) -> Self {
        Self {
            pid: pid.to_string(),
            tid: tid.to_string(),
            r#type,
        }
    }
}

impl std::fmt::Display for OpKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{} ({})", self.pid, self.tid, self.r#type.as_ref())
    }
}

enum Claim {
    /// A job is running the operation right now.
    Running,
    /// The last job ended without closing the record. `attempts` counts the
    /// liveness re-drives since, and `due` is when the next one is allowed —
    /// the record is a safety net, not a spin loop: a task that cannot
    /// progress yet (or at all) must not poll its store every tick.
    Stalled { attempts: u32, due: Instant },
}

/// The operations this engine owns, and where each one stands. One entry per
/// operation that is *either* running or stalled; an operation that closed its
/// record leaves none.
#[derive(Default)]
pub(crate) struct OpClaims {
    claims: Mutex<HashMap<OpKey, Claim>>,
    /// Longest a stalled operation waits between re-drives; the window doubles
    /// from [`Self::backoff`] per attempt up to this cap.
    max_backoff: Duration,
}

impl OpClaims {
    /// Longest a re-driven operation waits for its next attempt.
    const MAX_BACKOFF: Duration = Duration::from_secs(300);

    pub(crate) fn new() -> Self {
        Self {
            claims: Mutex::new(HashMap::new()),
            max_backoff: Self::MAX_BACKOFF,
        }
    }

    /// Take the operation for a job that is starting it now, reporting how
    /// many liveness attempts it already has: the job carries that count
    /// through its own release, so a record that keeps failing to close backs
    /// off across re-drives instead of restarting the window each time.
    fn start(&self, key: OpKey) -> u32 {
        let mut claims = self.claims.lock();
        let attempts = match claims.get(&key) {
            Some(Claim::Stalled { attempts, .. }) => *attempts,
            _ => 0,
        };
        claims.insert(key, Claim::Running);
        attempts
    }

    /// Release the operation: `closed` says whether the job reached the
    /// effect's end state (the record's close was queued). A job that did not
    /// leaves the operation stalled, which is what the liveness pass drains.
    fn finish(&self, key: &OpKey, closed: bool, attempts: u32, backoff: Duration) {
        let mut claims = self.claims.lock();
        if closed {
            claims.remove(key);
            return;
        }
        let window = backoff
            .saturating_mul(1u32 << attempts.min(3))
            .min(self.max_backoff);
        claims.insert(
            key.clone(),
            Claim::Stalled {
                attempts,
                due: Instant::now() + window,
            },
        );
    }

    /// Whether a job is running this operation right now. A re-drive must
    /// never race one: the row it would replay belongs to that job.
    pub(crate) fn is_running(&self, key: &OpKey) -> bool {
        matches!(self.claims.lock().get(key), Some(Claim::Running))
    }

    /// Whether the liveness pass may try this operation now: it is stalled
    /// (or was never claimed through this registry, which is the case for a
    /// row left behind by a dead engine) and its backoff window is over.
    pub(crate) fn may_retry(&self, key: &OpKey, now: Instant) -> bool {
        match self.claims.lock().get(key) {
            None => true,
            Some(Claim::Running) => false,
            Some(Claim::Stalled { due, .. }) => *due <= now,
        }
    }

    /// Record a re-drive attempt and report how many this operation has had
    /// since its job last released it: the next attempt waits a doubled window
    /// (capped), so an operation that cannot progress is retried on a widening
    /// interval instead of once per tick.
    pub(crate) fn attempted(&self, key: &OpKey, backoff: Duration) -> u32 {
        let mut claims = self.claims.lock();
        if let Some(Claim::Running) = claims.get(key) {
            // the re-driven job already claimed it — its own release decides
            return 0;
        }
        let attempts = match claims.get(key) {
            Some(Claim::Stalled { attempts, .. }) => attempts.saturating_add(1),
            _ => 1,
        };
        let window = backoff
            .saturating_mul(1u32 << attempts.min(3))
            .min(self.max_backoff);
        claims.insert(
            key.clone(),
            Claim::Stalled {
                attempts,
                due: Instant::now() + window,
            },
        );
        attempts
    }

    /// How many liveness attempts this operation has had since its job last
    /// released it (test visibility for the escalation).
    #[cfg(test)]
    pub(crate) fn attempts(&self, key: &OpKey) -> u32 {
        match self.claims.lock().get(key) {
            Some(Claim::Stalled { attempts, .. }) => *attempts,
            _ => 0,
        }
    }

    /// Forget an operation the pass settled itself (its propagation is
    /// durable, or its record describes nothing left to do). Only ever called
    /// for an operation no job holds — a running claim belongs to its job.
    pub(crate) fn clear(&self, key: &OpKey) {
        if let Some(Claim::Running) = self.claims.lock().get(key) {
            return;
        }
        self.claims.lock().remove(key);
    }

    /// Drop the claims of a process the engine no longer holds in memory. Its
    /// rows are swept with it, so nothing is left to re-drive; a process that
    /// merely lost its resident slot keeps its claims — the outbox pass still
    /// finds its rows by scanning the store.
    pub(crate) fn retain_processes(&self, resident: &dyn Fn(&str) -> bool) {
        self.claims.lock().retain(|key, _| resident(&key.pid));
    }

    /// Whether any operation is stalled — a job ended without closing its
    /// record. This is the outbox pass's own "is there anything to look at?"
    /// question, so an engine with nothing stranded does not pay the store
    /// scan every tick.
    pub(crate) fn has_stalled(&self) -> bool {
        self.claims
            .lock()
            .values()
            .any(|claim| matches!(claim, Claim::Stalled { .. }))
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.claims.lock().len()
    }
}

/// One lane job's claim on the durable operation it runs.
///
/// The guard is installed before the effect starts and released when the job
/// ends — the normal path, a caught panic, an early return, or a dropped
/// future all reach [`Drop`]. Only a job that reached its operation's end
/// state calls [`Self::close`]; everything else leaves the operation stalled
/// for the liveness pass.
pub(crate) struct OpClaim {
    claims: Arc<OpClaims>,
    key: OpKey,
    backoff: Duration,
    /// Liveness attempts this operation already had when the job took it,
    /// carried into its release so the backoff keeps widening.
    attempts: u32,
    closed: bool,
}

impl OpClaim {
    /// Claim the operation `r#type` of `pid`'s task `tid`, releasing it on drop
    /// with the first liveness attempt `backoff` from now.
    pub(crate) fn start(
        claims: &Arc<OpClaims>,
        pid: &str,
        tid: &str,
        r#type: OpType,
        backoff: Duration,
    ) -> Self {
        let key = OpKey::new(pid, tid, r#type);
        let attempts = claims.start(key.clone());
        Self {
            claims: claims.clone(),
            key,
            backoff,
            attempts,
            closed: false,
        }
    }

    /// The job reached the operation's end state (its record close was
    /// queued). Releasing a closed operation drops its claim entirely; the
    /// durable row is the record of what happened.
    pub(crate) fn close(&mut self) {
        self.closed = true;
    }
}

impl Drop for OpClaim {
    fn drop(&mut self) {
        self.claims
            .finish(&self.key, self.closed, self.attempts, self.backoff);
    }
}

/// Whether some task under `task` is still in flight, and therefore still able
/// to re-enter it: a descendant's completion recurses into its ancestors'
/// `next`, which is what closes their open outbox records. A task whose
/// subtree is entirely terminal has no such path left, so an open record of it
/// is only finished by the liveness pass.
pub(crate) fn has_live_descendant(task: &Arc<Task>) -> bool {
    let mut stack = task.children();
    while let Some(t) = stack.pop() {
        if !t.state().is_completed() {
            return true;
        }
        stack.extend(t.children());
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TaskState, scheduler::Runtime, utils};

    fn key(tid: &str) -> OpKey {
        OpKey::new("p1", tid, OpType::Next)
    }

    fn start(claims: &Arc<OpClaims>, pid: &str, tid: &str, backoff: Duration) -> OpClaim {
        OpClaim::start(claims, pid, tid, OpType::Next, backoff)
    }

    /// A job that closed its record releases the operation entirely; one that
    /// did not leaves it stalled, which is what the liveness pass looks for.
    #[test]
    fn a_closed_claim_is_released_an_open_one_stalls() {
        let claims = Arc::new(OpClaims::new());
        let backoff = Duration::from_millis(10);

        {
            let mut claim = start(&claims, "p1", "t1", backoff);
            assert!(claims.is_running(&key("t1")));
            claim.close();
        }
        assert_eq!(claims.len(), 0, "a closed operation left a claim behind");

        {
            let _claim = start(&claims, "p1", "t2", backoff);
            assert!(claims.is_running(&key("t2")));
        }
        assert_eq!(claims.len(), 1);
        let abandoned = !claims.may_retry(&key("t2"), Instant::now());
        assert!(
            abandoned,
            "a just-abandoned operation must wait out its backoff window"
        );
        assert!(claims.may_retry(&key("t2"), Instant::now() + backoff));
    }

    /// The re-drive window widens per attempt, so an operation that cannot make
    /// progress stops being retried once per tick. The base is deliberately
    /// long compared with the margins asserted here, so the window's growth —
    /// not the clock's resolution — is what the comparison sees.
    #[test]
    fn attempts_widen_the_window() {
        let claims = OpClaims::new();
        let backoff = Duration::from_secs(1);
        let key = key("t1");

        // one attempt: the window is twice the base
        assert_eq!(claims.attempted(&key, backoff), 1);
        assert!(!claims.may_retry(&key, Instant::now() + backoff));
        assert!(claims.may_retry(&key, Instant::now() + backoff * 3));

        // two attempts: four times the base
        assert_eq!(claims.attempted(&key, backoff), 2);
        assert!(!claims.may_retry(&key, Instant::now() + backoff * 3));
        assert!(claims.may_retry(&key, Instant::now() + backoff * 6));

        // the window is capped, so a stuck operation still gets retried
        for _ in 0..8 {
            claims.attempted(&key, backoff);
        }
        assert!(claims.may_retry(&key, Instant::now() + OpClaims::MAX_BACKOFF));
    }

    /// A running claim is never a re-drive target, and the pass cannot drop it
    /// either: the row belongs to the job holding it.
    #[test]
    fn a_running_claim_is_never_retried_or_cleared() {
        let claims = Arc::new(OpClaims::new());
        let _claim = start(&claims, "p1", "t1", Duration::ZERO);

        let key = key("t1");
        assert!(claims.is_running(&key));
        assert!(!claims.may_retry(&key, Instant::now() + Duration::from_secs(3600)));
        claims.clear(&key);
        assert!(
            claims.is_running(&key),
            "a live claim must survive the pass"
        );
    }

    /// A job that takes an operation the pass had already retried keeps the
    /// attempt count, so the backoff does not restart from the first window
    /// every time a failed re-drive runs.
    #[test]
    fn a_re_driven_job_carries_the_attempts_through() {
        let claims = Arc::new(OpClaims::new());
        let backoff = Duration::from_millis(10);
        let key = key("t1");
        assert_eq!(claims.attempted(&key, backoff), 1);
        assert_eq!(claims.attempted(&key, backoff), 2);

        {
            let _claim = start(&claims, "p1", "t1", backoff);
            assert!(
                claims.is_running(&key),
                "starting supersedes the stall note"
            );
        }
        assert_eq!(
            claims.attempts(&key),
            2,
            "the release of an open re-drive keeps counting"
        );
    }

    /// Claims of a process the engine no longer holds are dropped: its rows go
    /// with it.
    #[test]
    fn claims_of_an_evicted_process_are_dropped() {
        let claims = Arc::new(OpClaims::new());
        let _a = start(&claims, "p1", "t1", Duration::ZERO);
        let _b = start(&claims, "p2", "t2", Duration::ZERO);
        assert_eq!(claims.len(), 2);

        claims.retain_processes(&|pid| pid == "p1");
        assert_eq!(claims.len(), 1);
        assert!(claims.is_running(&key("t1")));
        assert!(!claims.is_running(&OpKey::new("p2", "t2", OpType::Next)));
    }

    /// A task whose subtree is entirely terminal has no path left that
    /// re-enters it; one with an in-flight descendant does.
    #[tokio::test(flavor = "multi_thread")]
    async fn only_a_fully_terminal_subtree_has_no_live_re_entry() {
        let config = crate::Config::default();
        let runtime = crate::scheduler::Runtime::new(&config, None).unwrap();
        let workflow = crate::Workflow::new().with_step(|step| step.with_id("s1"));
        let proc = runtime.create_proc("p1", &workflow);
        let (root, step) = {
            let tree = proc.tree();
            let root = proc.create_task(tree.root.as_ref().unwrap(), None).unwrap();
            let step = proc
                .create_task(&tree.node("s1").unwrap(), Some(root.clone()))
                .unwrap();
            (root, step)
        };
        root.set_state(crate::TaskState::Running);
        step.set_state(crate::TaskState::Running);
        assert!(has_live_descendant(&root));
        assert!(!has_live_descendant(&step));

        step.set_state(crate::TaskState::Skipped);
        assert!(!has_live_descendant(&root));
    }

    /// Which task states leave a `next` pass work to do: the re-drive
    /// predicate behind the outbox pass. A state that waits on the outside
    /// world must not be re-driven — its open record is the contract — and a
    /// terminal task that already propagated must not be either (its record
    /// only needs closing, which the pass does without a pass run, and a
    /// terminal task is never replayed at all).
    #[tokio::test(flavor = "multi_thread")]
    async fn only_a_running_auto_completing_task_is_drivable() {
        let config = crate::Config::default();
        let runtime = Runtime::new(&config, None).unwrap();
        let workflow = crate::Workflow::new().with_step(|step| step.with_id("s1"));
        let proc = runtime.create_proc(&utils::longid(), &workflow);
        let step = {
            let tree = proc.tree();
            let root = proc.create_task(tree.root.as_ref().unwrap(), None).unwrap();
            proc.create_task(&tree.node("s1").unwrap(), Some(root))
                .unwrap()
        };

        for waiting in [
            TaskState::None,
            TaskState::Ready,
            TaskState::Pending,
            TaskState::Interrupt,
        ] {
            step.set_state(waiting.clone());
            assert!(
                !step.next_is_drivable(),
                "a task in {waiting} waits on the outside world"
            );
        }

        step.set_state(TaskState::Running);
        assert!(
            step.next_is_drivable(),
            "a running task can complete itself"
        );

        for terminal in [
            TaskState::Completed,
            TaskState::Submitted,
            TaskState::Skipped,
            TaskState::Error,
            TaskState::Cancelled,
            TaskState::Backed,
            TaskState::Removed,
            TaskState::Aborted,
        ] {
            step.set_state(terminal.clone());
            step.set_propagation_phase(crate::scheduler::PropagationPhase::None);
            assert!(
                !step.next_is_drivable(),
                "a terminal task in {terminal} is closed, never replayed: a re-run could schedule work a decision already resolved"
            );
        }
    }
}
