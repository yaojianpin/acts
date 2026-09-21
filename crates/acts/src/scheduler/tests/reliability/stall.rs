//! The outbox hand-over cells: a propagation record whose job ended without
//! closing it.
//!
//! A `next` outbox row is created before its propagation is dispatched and
//! closed only after the effect is durable. Everything in between is the
//! window this file pins:
//!
//! - **the write cut.** The row's close is queued, but the lifecycle row
//!   carrying the terminal state and the scope vars row carrying the applied
//!   propagation marker are separate store writes (and the marker's row can be
//!   flushed by a *descendant's* persist), so a crash can leave `applied`
//!   beside a non-terminal state — a pair in which the propagation genuinely
//!   never finished, because the parent was never completed. Reading the
//!   marker alone closed the record and stranded the parent chain forever: the
//!   process kept its resident slot and every parked process waited behind it.
//! - **the abandoned job.** A job that ends without closing its record (a
//!   panic, a cancelled future, a queue that refused the dispatch, a close
//!   whose write was lost) leaves exactly the row a crash does, and only the
//!   next engine start used to re-drive it.
//!
//! The cells below drive both: the first sweeps the durable cut across every
//! write boundary of a completion cascade and requires the run to converge
//! from each one, and the next three strand the exact reported state — the
//! child's record `done`, its parent's and the root's
//! `pending`/`effect_in_flight` with nothing running them — and require the
//! runtime outbox pass (no restart), the periodic timer, and a second restart
//! to finish it.

use super::*;
use crate::StoreGuard;
use crate::{
    TaskState,
    event::{Action, EventAction},
    scheduler::{NodeKind, PropagationPhase, Runtime},
};
use serial_test::serial;
use std::sync::atomic::{AtomicBool, AtomicUsize};

/// The workflow every cell uses: one step that `uses` an IRQ act, so a
/// completion walks the full `act → step → root` chain and each level owns a
/// `next` outbox record.
fn irq_workflow() -> Workflow {
    Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    })
}

/// KV backend that applies writes until a budget runs out and then refuses
/// every one: a crash whose durable cut lands at one exact write. Reads keep
/// answering — the rows that landed are the crashed process's durable state.
struct CutKv {
    inner: Arc<MemoryStore>,
    armed: AtomicBool,
    budget: AtomicUsize,
    cut: AtomicBool,
    /// What a write past the cut does: `true` drops it silently (the process
    /// died — its last writes never reached the disk, and the engine that made
    /// them believes they did), `false` reports a store failure (the backend
    /// refuses the write while the engine keeps running).
    drop_writes: AtomicBool,
}

impl CutKv {
    fn new(inner: Arc<MemoryStore>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            armed: AtomicBool::new(false),
            budget: AtomicUsize::new(0),
            cut: AtomicBool::new(false),
            drop_writes: AtomicBool::new(false),
        })
    }

    /// Let `budget` more writes land, then stop applying any and report a
    /// store failure for each.
    fn arm(&self, budget: usize) {
        self.drop_writes.store(false, Ordering::SeqCst);
        self.budget.store(budget, Ordering::SeqCst);
        self.armed.store(true, Ordering::SeqCst);
    }

    /// Let `budget` more writes land, then silently drop every later one: the
    /// crash a real process death is — the writes are gone, and whoever made
    /// them got no error (so a boot over this store still succeeds).
    fn arm_drop(&self, budget: usize) {
        self.drop_writes.store(true, Ordering::SeqCst);
        self.budget.store(budget, Ordering::SeqCst);
        self.armed.store(true, Ordering::SeqCst);
    }

    fn cut(&self) -> bool {
        self.cut.load(Ordering::SeqCst)
    }

    /// Whether this write lands.
    fn allow(&self) -> bool {
        if self.cut.load(Ordering::SeqCst) {
            return false;
        }
        if !self.armed.load(Ordering::SeqCst) {
            return true;
        }
        if self
            .budget
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_err()
        {
            return false;
        }
        if self.budget.load(Ordering::SeqCst) == 0 {
            self.cut.store(true, Ordering::SeqCst);
        }
        true
    }

    /// The result a refused write reports: an error, or the silent success of
    /// a write whose process is already gone (`true`: nothing was refused).
    fn refused(&self) -> crate::Result<bool> {
        if self.drop_writes.load(Ordering::SeqCst) {
            Ok(true)
        } else {
            Err(ActError::Store("cut: the process crashed".into()))
        }
    }
}

#[async_trait::async_trait]
impl KvStore for CutKv {
    async fn one(&self, key: &str) -> crate::Result<Option<Vec<u8>>> {
        self.inner.one(key).await
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> crate::Result<()> {
        if !self.allow() {
            return self.refused().map(|_| ());
        }
        self.inner.put(key, value).await
    }

    async fn delete(&self, key: &str) -> crate::Result<()> {
        if !self.allow() {
            return self.refused().map(|_| ());
        }
        self.inner.delete(key).await
    }

    async fn batch(&self, ops: &[StoreBatchOp], guards: &[StoreGuard]) -> crate::Result<bool> {
        if !self.allow() {
            return self.refused();
        }
        self.inner.batch(ops, guards).await
    }

    async fn scan_prefix(
        &self,
        key: &str,
        options: ScanOptions,
    ) -> crate::Result<Vec<(String, Vec<u8>)>> {
        self.inner.scan_prefix(key, options).await
    }
}

/// The stored rows of every pid that still has a process row, and of the pids
/// given: printable evidence for a failing cell.
async fn rows_of(store: &Arc<crate::store::Store>, pids: &[String]) -> String {
    let mut out = String::new();
    for pid in pids {
        let q = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.clone())));
        let tasks = store.tasks().query(&q).await.unwrap().rows;
        let ops = store.ops().query(&q).await.unwrap().rows;
        if tasks.is_empty() && ops.is_empty() && store.procs().find(pid).await.is_err() {
            continue;
        }
        out.push_str(&format!(
            "proc {pid} state={:?}\n",
            store.procs().find(pid).await.map(|p| p.state)
        ));
        for t in tasks {
            out.push_str(&format!(
                "  task {} kind={} state={} parent={:?}\n",
                t.tid, t.kind, t.state, t.parent
            ));
        }
        for v in store.vars().query(&q).await.unwrap().rows {
            out.push_str(&format!("  vars {} data={}\n", v.tid, v.data));
        }
        for o in ops {
            out.push_str(&format!(
                "  op {} tid={} target={:?} status={} phase={} ver={}\n",
                o.r#type, o.tid, o.target_tid, o.status, o.phase, o.source_version
            ));
        }
    }
    out
}

/// Whether `pid` still has a durable process row, and whether it is terminal.
async fn process_row_is(store: &Arc<crate::store::Store>, pid: &str, completed: bool) -> bool {
    match store.procs().find(pid).await {
        Ok(row) => TaskState::from(row.state.as_str()).is_completed() == completed,
        Err(_) => !completed,
    }
}

/// The pids of `pids` whose process row is still stored.
async fn unfinished(store: &Arc<crate::store::Store>, pids: &[String]) -> Vec<String> {
    let mut left = Vec::new();
    for pid in pids {
        if store.procs().find(pid).await.is_ok() {
            left.push(pid.clone());
        }
    }
    left
}

/// Drive the sweeper until every one of `pids` is gone, then assert that no
/// row family of those processes is left behind — a finished run takes its
/// proc, task, vars, message, delivery and outbox rows with it.
async fn sweep_and_assert_clean(rt: &Arc<Runtime>, pids: &[String]) {
    let store = rt.cache().store();
    let done = poll_until("every swept process's rows to be gone", || async {
        let _ = rt.cache().sweep_removable().await;
        unfinished(&store, pids).await.is_empty()
    })
    .await;
    let left = unfinished(&store, pids).await;
    assert!(
        done && left.is_empty(),
        "these processes were never swept: {left:?}\n{}",
        rows_of(&store, &left).await
    );
    let q = Query::new().filter(Filter::and().expr(Expr::r#in("pid", pids.to_vec())));
    assert!(
        store.tasks().query(&q).await.unwrap().rows.is_empty(),
        "task rows outlived their swept process"
    );
    assert!(
        store.vars().query(&q).await.unwrap().rows.is_empty(),
        "vars rows outlived their swept process"
    );
    assert!(
        store.ops().query(&q).await.unwrap().rows.is_empty(),
        "outbox rows outlived their swept process"
    );
    assert!(
        store.messages().query(&q).await.unwrap().rows.is_empty(),
        "message rows outlived their swept process"
    );
    assert!(
        store.deliveries().query(&q).await.unwrap().rows.is_empty(),
        "delivery rows outlived their swept process"
    );
}

/// The tid of the waiting IRQ act of `pid`, if it has one.
async fn waiting_act(store: &Arc<crate::store::Store>, pid: &str) -> Option<String> {
    let q = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.to_string())));
    store
        .tasks()
        .query(&q)
        .await
        .unwrap()
        .rows
        .into_iter()
        .find(|t| t.state == TaskState::Interrupt.to_string())
        .map(|t| t.tid)
}

/// Complete one waiting act of `pid`'s resident process, if there is one.
async fn complete_waiting_act(rt: &Arc<Runtime>, pid: &str) -> bool {
    let store = rt.cache().store();
    let Some(tid) = waiting_act(&store, pid).await else {
        return false;
    };
    rt.do_action(&Action::new(pid, &tid, EventAction::Next, Vars::new()))
        .await
        .is_ok()
}

/// Drive every process to its end: complete each waiting act as it appears,
/// failing (never hanging) if nothing progresses for a full deadline.
async fn drive_to_end(rt: &Arc<Runtime>, pids: &[String]) {
    let store = rt.cache().store();
    let mut last = unfinished(&store, pids).await.len();
    let mut last_progress = Instant::now();
    loop {
        let left = unfinished(&store, pids).await;
        if left.is_empty() {
            return;
        }
        if left.len() < last {
            last = left.len();
            last_progress = Instant::now();
        }
        assert!(
            last_progress.elapsed() < WAIT,
            "the run stalled: {} of {} processes never finished\ncapacity(resident, reserved)={:?}\nresident={:?}\nqueued_resume={:?}\n{}",
            left.len(),
            pids.len(),
            rt.cache().capacity_state(),
            rt.cache()
                .procs()
                .iter()
                .map(|p| format!("{}:{}", p.id(), p.state()))
                .collect::<Vec<_>>(),
            rt.cache().pending_resume_ids(),
            rows_of(&store, &left).await
        );
        let mut acted = false;
        for pid in pids {
            if complete_waiting_act(rt, pid).await {
                acted = true;
                break;
            }
        }
        if !acted {
            tokio::time::sleep(TICK).await;
        }
    }
}

/// Start `count` processes over `rt`'s store — `cap` of them resident, the rest
/// parked — and wait until `resident` of them have a persisted waiting IRQ act.
async fn start_irq_processes(
    rt: &Arc<Runtime>,
    workflow: &Workflow,
    prefix: &str,
    count: usize,
    resident: usize,
) -> Vec<String> {
    let store = rt.cache().store();
    let mut pids = Vec::with_capacity(count);
    for i in 0..count {
        let pid = format!("{prefix}{i}");
        let proc = rt.create_proc(&pid, workflow);
        rt.launch(&proc).await.unwrap();
        pids.push(pid);
    }
    let reached = poll_until("the resident processes' IRQ acts to be waiting", || async {
        let mut waiting = 0;
        for pid in &pids {
            if waiting_act(&store, pid).await.is_some() {
                waiting += 1;
            }
        }
        waiting >= resident
    })
    .await;
    assert!(
        reached,
        "{resident} of {count} processes never reached their IRQ act"
    );
    pids
}

/// The `pid` whose process is resident and has a waiting IRQ act.
async fn first_waiting(store: &Arc<crate::store::Store>, pids: &[String]) -> Option<String> {
    for pid in pids {
        if waiting_act(store, pid).await.is_some() {
            return Some(pid.clone());
        }
    }
    None
}

/// The cut is swept across the write boundary of a completion cascade: the
/// client action that drives `act → step → root` is applied while the store
/// stops applying writes at exactly write #k. Whatever the cut leaves durable
/// — including the state where the act's record is `done` while its step and
/// the root are `running` with their records still
/// `pending`/`effect_in_flight` — the reloaded engine must finish every
/// process, start every parked one, and sweep all of their rows.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_next_cut_sweep_recovers_every_write_boundary() {
    bounded(
        "sch_next_cut_sweep_recovers_every_write_boundary",
        cut_sweep_inner(),
    )
    .await;
}

/// Write boundaries swept. The cascade this drives is ~20 store writes long
/// (task rows, vars rows, outbox rows, phase notes, the proc row), so the
/// sweep covers it with room to spare.
const CUT_SWEEP: usize = 40;

async fn cut_sweep_inner() {
    let workflow = irq_workflow();
    for cut_at in 0..CUT_SWEEP {
        let inner = Arc::new(MemoryStore::new());
        let kv = CutKv::new(inner.clone());
        let store_kv: Arc<dyn KvStore> = kv.clone();
        let prefix = format!("cut{cut_at}_");
        let pids;
        {
            let engine = Engine::builder()
                .cache_size(2)
                .set_store(store_kv)
                .start()
                .await
                .unwrap();
            let rt = engine.runtime();
            pids = start_irq_processes(&rt, &workflow, &prefix, 8, 2).await;
            let store = rt.cache().store();
            // complete one waiting act; the cut lands inside its cascade
            let pid = first_waiting(&store, &pids)
                .await
                .expect("a resident process with a waiting act");
            kv.arm(cut_at);
            let _ = complete_waiting_act(&rt, &pid).await;
            // The window closes as soon as the cut fires or the armed process
            // reaches its terminal state. Waiting longer would let the
            // periodic timers — and the processes started in between — write
            // their own state, so what the cut leaves behind would depend on
            // timing instead of on the write index.
            let _ = poll_settle(
                || async { kv.cut() || !process_row_is(&store, &pid, false).await },
                50,
            )
            .await;
            engine.close().await;
        }

        // reload over the rows the cut left and drive the acts to the end
        let engine = Engine::builder()
            .cache_size(2)
            .set_store(inner.clone())
            .start()
            .await
            .unwrap();
        let rt = engine.runtime();
        drive_to_end(&rt, &pids).await;
        sweep_and_assert_clean(&rt, &pids).await;
        engine.close().await;
    }
}

/// A `next` record stranded at `pending`/`effect_in_flight` — exactly the
/// reported state: the IRQ act's record `done`, its step's and the root's
/// records open with no job owning them — is re-driven by the engine's own
/// outbox pass, with **no restart**: the step completes, its completion
/// recurses into the root, the process finishes and every row of it is swept.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_next_stalled_op_is_redriven_by_the_outbox_pass() {
    bounded(
        "sch_next_stalled_op_is_redriven_by_the_outbox_pass",
        stalled_redriven_inner(),
    )
    .await;
}

async fn stalled_redriven_inner() {
    let engine = Engine::builder().start().await.unwrap();
    let rt = engine.runtime();
    let workflow = irq_workflow();
    let pids = start_irq_processes(&rt, &workflow, "stall", 1, 1).await;
    let pid = pids[0].clone();
    strand_completed_act(&rt, &pid).await;

    // nothing owns the step's record: the pass has to re-drive it
    rt.recover_outbox(0).await.unwrap();
    let finished = poll_until(
        "the outbox pass to finish the stalled propagation",
        || async {
            rt.cache()
                .store()
                .procs()
                .find(&pid)
                .await
                .map(|p| TaskState::from(p.state.as_str()).is_completed())
                .unwrap_or(true)
        },
    )
    .await;
    assert!(
        finished,
        "the stalled propagation never finished\n{}",
        rows_of(&rt.cache().store(), &pids).await
    );
    sweep_and_assert_clean(&rt, &pids).await;
    engine.close().await;
}

/// The same stranded state, healed by the periodic timer instead of a direct
/// pass call: the engine re-drives an abandoned `next` record on its own, at
/// runtime, so a stranded propagation is not a restart-shaped hole in the
/// outbox contract.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_next_stalled_op_is_redriven_by_the_timer() {
    bounded(
        "sch_next_stalled_op_is_redriven_by_the_timer",
        stalled_timer_inner(),
    )
    .await;
}

async fn stalled_timer_inner() {
    let engine = Engine::builder().start().await.unwrap();
    let rt = engine.runtime();
    let workflow = irq_workflow();
    let pids = start_irq_processes(&rt, &workflow, "timer", 1, 1).await;
    let pid = pids[0].clone();
    strand_completed_act(&rt, &pid).await;

    // the retry timer's own tick (the test tick is 800ms) carries the pass
    let finished = poll_until("the timer to finish the stalled propagation", || async {
        rt.cache()
            .store()
            .procs()
            .find(&pid)
            .await
            .map(|p| TaskState::from(p.state.as_str()).is_completed())
            .unwrap_or(true)
    })
    .await;
    assert!(
        finished,
        "the timer never re-drove the stalled propagation\n{}",
        rows_of(&rt.cache().store(), &pids).await
    );
    sweep_and_assert_clean(&rt, &pids).await;
    engine.close().await;
}

/// Two restarts over the same stranded work: the crash that leaves the
/// parents' records open, a second engine whose own recovery is cut as well,
/// and a third that must carry the run to its end — with every parked process
/// started and every row family swept.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_next_stalled_op_survives_a_second_restart() {
    bounded(
        "sch_next_stalled_op_survives_a_second_restart",
        second_restart_inner(),
    )
    .await;
}

async fn second_restart_inner() {
    let workflow = irq_workflow();
    let inner = Arc::new(MemoryStore::new());
    let kv = CutKv::new(inner.clone());
    let store_kv: Arc<dyn KvStore> = kv.clone();
    let pids;
    {
        let engine = Engine::builder()
            .cache_size(2)
            .set_store(store_kv.clone())
            .start()
            .await
            .unwrap();
        let rt = engine.runtime();
        pids = start_irq_processes(&rt, &workflow, "twice", 4, 2).await;
        let store = rt.cache().store();
        let pid = first_waiting(&store, &pids)
            .await
            .expect("a resident process with a waiting act");
        // the first cut lands inside the completion cascade (the write that
        // carries the step's terminal state is one of the first few)
        kv.arm_drop(4);
        let _ = complete_waiting_act(&rt, &pid).await;
        let _ = poll_settle(|| async { kv.cut() }, 50).await;
        engine.close().await;
    }

    // second engine: its recovery runs against a store that stops accepting
    // writes after two of them, so the run is cut a second time mid-recovery
    {
        kv.arm_drop(2);
        let engine = Engine::builder()
            .cache_size(2)
            .set_store(store_kv)
            .start()
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        engine.close().await;
    }

    // third engine over the whole store: the run must finish
    let engine = Engine::builder()
        .cache_size(2)
        .set_store(inner.clone())
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();
    drive_to_end(&rt, &pids).await;
    sweep_and_assert_clean(&rt, &pids).await;
    engine.close().await;
}

/// Take the process to the reported durable state: the IRQ act is completed
/// and its propagation applied (its outbox record closed), while its step and
/// the root stay `running` with their `next` records `pending` at
/// `effect_in_flight` and no job owning them.
async fn strand_completed_act(rt: &Arc<Runtime>, pid: &str) {
    let proc = rt
        .proc(pid)
        .await
        .unwrap()
        .expect("the process is resident");
    let act = proc
        .tasks()
        .into_iter()
        .find(|t| t.is_kind(NodeKind::Act) && t.state().is_interrupted())
        .expect("the IRQ act is waiting");
    let step = act.parent().expect("the act has its step");
    let root = proc.root().expect("the process has its root");

    // the act's propagation ran to completion before the crash: terminal state
    // and applied marker durable, outbox record closed
    act.set_state(TaskState::Completed);
    act.set_propagation_phase(PropagationPhase::Applied);
    let store = rt.cache().store();
    store.upsert_task(&act).await.unwrap();
    store.complete_ops(pid, &act.id, "next").await.unwrap();

    // the parents are exactly where the cut left them: running, their records
    // open and claimed by a job that is gone
    assert!(step.state().is_running());
    assert!(root.state().is_running());
    for task in [&step, &root] {
        let q = Query::new().filter(
            Filter::and()
                .expr(Expr::eq("pid", pid.to_string()))
                .expr(Expr::eq("tid", task.id.clone())),
        );
        let rows = store.ops().query(&q).await.unwrap().rows;
        let next = rows
            .iter()
            .find(|op| op.r#type == "next")
            .unwrap_or_else(|| panic!("task {} must own a next record", task.id));
        assert_eq!(next.status, "pending");
        assert_eq!(next.phase, "effect_in_flight");
    }
}

/// The boot replay must never grow a process the resident set does not hold.
/// `cache.proc` answers a full set with a loaded-but-uncached instance; a
/// record replayed on it schedules work into a tree nothing else will ever
/// see — the durable rows land, but the process a resume loads as *the*
/// resident never owns the scheduling, its records are then closed as
/// orphans ("no task"), and the run strands on tasks only the store knows
/// about. The replay defers such a record instead: it stays exactly as
/// stored, and is driven once the process is resident.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_next_boot_replay_defers_a_non_resident_process() {
    bounded(
        "sch_next_boot_replay_defers_a_non_resident_process",
        boot_replay_defers_non_resident_inner(),
    )
    .await;
}

async fn boot_replay_defers_non_resident_inner() {
    let workflow = irq_workflow();
    let store_kv: Arc<dyn KvStore> = Arc::new(MemoryStore::new());

    // first boot, resident cap 1: one live process holds the slot
    {
        let engine = Engine::builder()
            .cache_size(1)
            .set_store(store_kv.clone())
            .start()
            .await
            .unwrap();
        let rt = engine.runtime();
        start_irq_processes(&rt, &workflow, "ghost-a", 1, 1).await;

        // a second in-flight process, crafted straight into the store: a
        // running root with a pending `next` record — the shape a crash
        // mid-propagation leaves behind
        let proc = rt.create_proc("ghost-b", &workflow);
        proc.set_state(TaskState::Running);
        let root = {
            let tree = proc.tree();
            proc.create_task(tree.root.as_ref().unwrap(), None).unwrap()
        };
        root.set_state(TaskState::Running);
        let store = rt.cache().store();
        store.upsert_proc(&proc).await.unwrap();
        store.upsert_task(&root).await.unwrap();
        store.enqueue_next_op(proc.id(), &root.id).await.unwrap();
        engine.close().await;
    }

    // second boot over the same store: the live process is resumed into the
    // single slot and the second process waits in the boot-resume overflow
    let engine = Engine::builder()
        .cache_size(1)
        .set_store(store_kv)
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();
    let store = rt.cache().store();
    let pid = "ghost-b".to_string();
    let q_pid = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.clone())));

    // the replay runs while the resident set is full: the record must come
    // out untouched, and no task may be scheduled for the process
    rt.recover_actions().await.unwrap();
    assert!(
        store
            .ops()
            .query(&q_pid)
            .await
            .unwrap()
            .rows
            .iter()
            .any(|op| op.r#type == "next" && op.status == "pending"),
        "the deferred record must stay open"
    );
    assert_eq!(
        store.tasks().query(&q_pid).await.unwrap().rows.len(),
        1,
        "a non-resident process must not gain tasks from the replay"
    );

    // once a slot frees, the process is resumed and the deferred record is
    // what carries it forward: the same replay now schedules its step
    complete_waiting_act(&rt, "ghost-a0").await;
    poll_until("the freed slot's process to be resumed", || async {
        rt.cache().resident(&pid).is_some()
    })
    .await;
    assert!(
        rt.cache().resident(&pid).is_some(),
        "the deferred process must be resumed into the freed slot"
    );
    rt.recover_actions().await.unwrap();
    let reloaded = rt
        .proc(&pid)
        .await
        .unwrap()
        .expect("the process is resident");
    let grew = poll_until(
        "the resumed process's deferred record to schedule its step",
        || async { reloaded.tasks().len() > 1 },
    )
    .await;
    assert!(
        grew && reloaded.tasks().len() > 1,
        "the resident process's deferred record must schedule its step"
    );

    // both processes run to their end and leave nothing behind
    let pids = vec!["ghost-a0".to_string(), pid];
    drive_to_end(&rt, &pids).await;
    sweep_and_assert_clean(&rt, &pids).await;
    engine.close().await;
}
