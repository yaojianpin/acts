//! Timeout-branch reliability under adverse conditions.
//!
//! A step's timeout branch is a one-shot: [`Task::claim_timeout`] takes the
//! slot under the scope lock and the claim is made durable before the branch's
//! effect. These tests pin that contract where it is under pressure:
//!
//! - a store fault on the claim's own write path must not lose the timeout,
//!   duplicate the branch, or leave the durable claim marker inconsistent;
//! - a client completion racing the tick that fires the branch must converge
//!   to one settled state with each branch fired exactly once.

use super::*;
use crate::{
    ActError, Action, Config, Engine, Message, MessageState, Vars, Workflow,
    event::EventAction,
    scheduler::{Process, Runtime, TaskState},
    store::{
        KvStore, MemoryStore, ScanOptions, StoreBatchOp, StoreIden,
        query::{Expr, Filter, Query},
    },
    utils::{
        self,
        consts::TASK_TIMEOUTS,
        test::{USES_IRQ, USES_MSG, create_proc},
    },
};
use serial_test::serial;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

/// Poll with a deadline until `cond` holds — the established convention for
/// settling asynchronous work (scheduler lanes, store writer) in tests.
async fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if cond() {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// KV backend that loses exactly one write: the first `batch` carrying the
/// timed-out task's scope vars row with the one-shot marker in it — the write
/// that is supposed to make a claim durable. Everything else passes through.
///
/// Targeting the row key plus the marker content (not "the first write after
/// arming") makes the injection deterministic against the launched process's
/// own tick timer and the writer's async applies.
struct TimeoutClaimFaultKv {
    inner: MemoryStore,
    /// Full data key of the owner task's vars row (`vars-id--<pid><tid>`),
    /// `None` until the test aims the fault.
    claim_key: parking_lot::Mutex<Option<String>>,
    /// Set once the fault fired.
    faulted: AtomicBool,
}

impl TimeoutClaimFaultKv {
    fn new() -> Self {
        Self {
            inner: MemoryStore::new(),
            claim_key: parking_lot::Mutex::new(None),
            faulted: AtomicBool::new(false),
        }
    }

    /// Aim the fault at one task's vars row.
    fn target(&self, pid: &str, tid: &str) {
        let row_id = utils::Id::new(pid, tid).id();
        *self.claim_key.lock() = Some(format!(
            "{}{}id{}{}",
            StoreIden::Vars.as_ref(),
            crate::utils::consts::KEY_SEP,
            crate::utils::consts::KEY_SEP,
            row_id
        ));
    }

    fn faulted(&self) -> bool {
        self.faulted.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl KvStore for TimeoutClaimFaultKv {
    async fn one(&self, key: &str) -> crate::Result<Option<Vec<u8>>> {
        self.inner.one(key).await
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> crate::Result<()> {
        self.inner.put(key, value).await
    }

    async fn delete(&self, key: &str) -> crate::Result<()> {
        self.inner.delete(key).await
    }

    async fn batch(&self, ops: &[StoreBatchOp]) -> crate::Result<()> {
        let claim_key = self.claim_key.lock().clone();
        let carries_claim = claim_key.is_some_and(|key| {
            ops.iter().any(|op| match op {
                StoreBatchOp::Put { key: k, value } if *k == key => {
                    String::from_utf8_lossy(value).contains(TASK_TIMEOUTS)
                }
                _ => false,
            })
        });
        // one shot: the first claim write is lost, every later one (the heal)
        // goes through
        if carries_claim && !self.faulted.swap(true, Ordering::SeqCst) {
            return Err(ActError::Store(
                "injected: backend lost the timeout claim write".to_string(),
            ));
        }
        self.inner.batch(ops).await
    }

    async fn scan_prefix(
        &self,
        key: &str,
        options: ScanOptions,
    ) -> crate::Result<Vec<(String, Vec<u8>)>> {
        self.inner.scan_prefix(key, options).await
    }
}

/// The branch's claim write hits a store fault while the branch fires: the
/// error must surface cleanly, the tick must not lose the timeout nor
/// duplicate the branch, and the durable claim marker must end up consistent
/// — so a restored process does not re-fire the branch.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_timeout_store_fault_fires_branch_exactly_once() {
    bounded(
        "sch_step_timeout_store_fault_fires_branch_exactly_once",
        sch_step_timeout_store_fault_fires_branch_exactly_once_inner(),
    )
    .await;
}

async fn sch_step_timeout_store_fault_fires_branch_exactly_once_inner() {
    let kv = Arc::new(TimeoutClaimFaultKv::new());
    let engine = Engine::builder()
        .set_store(kv.clone())
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();

    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_timeout(|timeout| {
                timeout
                    .with_id("timeout1")
                    .with_if("$cost() >= 0")
                    .with_uses(USES_MSG, Vars::new().with("key", "msg1"))
            })
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let pid = utils::longid();
    let proc = rt.create_proc(&pid, &workflow);

    // msg1 counts the branch's side effect; the act1 trap tells when step1 is
    // genuinely in flight (its irq act waiting for the client)
    let msgs = engine.signal(Vec::<Message>::default());
    let ready = engine.signal((String::new(), String::new()));
    let (ready_tx, ready_fill) = ready.double();
    let collector = msgs.clone();
    let filler = ready_fill.clone();
    engine.channel().on_message(move |e| {
        let collector = collector.clone();
        let filler = filler.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                filler.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                filler.close();
            }
            if e.is_params_key("msg1") {
                collector.update(|data| data.push(e.inner().clone()));
            }
        }
    });

    rt.launch(&proc).await.unwrap();
    let (_p, _act1_tid) = ready_tx.recv().await;
    let owner = proc.task_by_nid("step1").first().unwrap().clone();
    assert!(owner.state().is_running());

    // arm the fault on the claim write itself
    kv.target(&pid, &owner.id);

    proc.do_tick().await;

    wait_until("the injected claim-write fault", || kv.faulted()).await;
    wait_until("the timeout branch settled", || {
        proc.task_by_nid("timeout1")
            .first()
            .is_some_and(|t| t.state().is_completed())
    })
    .await;
    wait_until("the branch message delivered", || msgs.data().len() == 1).await;

    // the branch's init persisted through the async writer, so upsert_async
    // returned Ok — the failure is not swallowed though: the next flush
    // barrier acks it
    let err = rt.cache().flush().await.unwrap_err();
    assert!(matches!(err, ActError::Store(_)), "{err:?}");
    assert!(err.to_string().contains("injected"), "{err:?}");

    // the lost write did not consume the claim: in memory the branch has
    // fired, and repeated ticks neither re-dispatch it nor duplicate anything
    for _ in 0..3 {
        proc.do_tick().await;
    }
    rt.cache().flush().await.unwrap();
    assert_eq!(proc.task_by_nid("timeout1").len(), 1);
    assert_eq!(msgs.data().len(), 1);

    // heal: the failed write had not cleared the owner's dirty flag, so one
    // clean persist of the owner retries the vars row
    rt.cache().store().persist_task_rows(&owner).await.unwrap();
    rt.cache().flush().await.unwrap();

    // the durable claim marker ended up consistent with what fired
    let vars_row = rt
        .cache()
        .store()
        .vars()
        .find(&utils::Id::new(&pid, &owner.id).id())
        .await
        .unwrap();
    let data: Vars = serde_json::from_str(&vars_row.data).unwrap();
    assert_eq!(
        data.get::<Vec<String>>(TASK_TIMEOUTS).unwrap(),
        vec!["timeout1".to_string()],
        "the healed claim must be durable"
    );

    // a restored process reads the healed marker: the branch is not re-fired,
    // so the fault cost no duplication across the restart either
    engine.close().await;
    let rt2 = Runtime::new(&Config::default(), Some(kv.clone())).unwrap();
    let restored = rt2
        .cache()
        .store()
        .load_proc(&pid, &rt2)
        .await
        .unwrap()
        .unwrap();
    let restored_owner = restored.task_by_nid("step1").first().unwrap().clone();
    assert!(restored_owner.is_timeout_claimed("timeout1"));
    for _ in 0..3 {
        restored.do_tick().await;
    }
    assert_eq!(restored.task_by_nid("timeout1").len(), 1);
    assert_eq!(msgs.data().len(), 1, "no duplicate branch after restart");
    rt2.close().await;
}

/// `do_tick` racing a client completion of the same timing-out task's act:
/// however the interleaving goes, each timeout branch fires exactly once (the
/// claim invariant), the task settles into exactly one state with the store
/// agreeing, no duplicate rows appear, and the flow stays alive.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_timeout_racing_complete_converges_exactly_once() {
    bounded(
        "sch_step_timeout_racing_complete_converges_exactly_once",
        sch_step_timeout_racing_complete_converges_exactly_once_inner(),
    )
    .await;
}

async fn sch_step_timeout_racing_complete_converges_exactly_once_inner() {
    let workflow = Workflow::new()
        .with_step(|step| {
            step.with_id("step1")
                .with_timeout(|timeout| {
                    timeout
                        .with_id("timeout1")
                        .with_if("$cost() >= 0")
                        .with_uses(USES_MSG, Vars::new().with("key", "msg1"))
                })
                .with_timeout(|timeout| {
                    timeout
                        .with_id("timeout2")
                        .with_if("$cost() >= 0")
                        .with_uses(USES_MSG, Vars::new().with("key", "msg2"))
                })
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
        })
        .with_step(|step| {
            step.with_id("step2")
                .with_uses(USES_IRQ, Vars::new().with("key", "act2"))
        });

    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let rt = engine.runtime();
    let pid = proc.id().to_string();

    let msgs1 = engine.signal(Vec::<Message>::default());
    let msgs2 = engine.signal(Vec::<Message>::default());
    let ready1 = engine.signal((String::new(), String::new()));
    let ready2 = engine.signal((String::new(), String::new()));
    let (r1tx, r1fill) = ready1.double();
    let (r2tx, r2fill) = ready2.double();
    let c1 = msgs1.clone();
    let c2 = msgs2.clone();
    let f1 = r1fill.clone();
    let f2 = r2fill.clone();
    engine.channel().on_message(move |e| {
        let c1 = c1.clone();
        let c2 = c2.clone();
        let f1 = f1.clone();
        let f2 = f2.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                f1.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                f1.close();
            }
            if e.is_params_key("act2") && e.is_state(MessageState::Created) {
                f2.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                f2.close();
            }
            if e.is_params_key("msg1") {
                c1.update(|data| data.push(e.inner().clone()));
            }
            if e.is_params_key("msg2") {
                c2.update(|data| data.push(e.inner().clone()));
            }
        }
    });

    rt.launch(&proc).await.unwrap();
    let (_, act1_tid) = r1tx.recv().await;

    // round 0: the tick alone fires the first branch — its one-shot claim is
    // taken deterministically before any racing begins
    proc.do_tick().await;
    wait_until("the first timeout branch settled", || {
        proc.task_by_nid("timeout1")
            .first()
            .is_some_and(|t| t.state().is_completed())
    })
    .await;
    assert_eq!(msgs1.data().len(), 1);

    // racing rounds: concurrent ticks against the client completing the same
    // task's act (step1's in-flight irq), then against the already-settled
    // task — every interleaving must stay inside the claim invariant
    for round in 0..4usize {
        let mut ticks = Vec::new();
        for _ in 0..3 {
            let p = proc.clone();
            ticks.push(tokio::spawn(async move { p.do_tick().await }));
        }
        let rtt = rt.clone();
        let action = Action::new(&pid, &act1_tid, EventAction::Next, Vars::new());
        let act = tokio::spawn(async move { rtt.do_action(&action).await });
        for tick in ticks {
            tick.await.unwrap();
        }
        let result = act.await.unwrap();
        if round == 0 {
            result.expect("the first racing completion must be accepted");
        } else {
            assert!(
                result.is_err(),
                "a completion of the already-settled act must be rejected"
            );
        }
    }

    wait_until("both timeout branches settled", || {
        ["timeout1", "timeout2"].iter().all(|nid| {
            let tasks = proc.task_by_nid(nid);
            !tasks.is_empty() && tasks.iter().all(|t| t.state().is_completed())
        })
    })
    .await;

    // keep racing after everything settled: the converged outcome must not
    // move
    for _ in 0..3 {
        proc.do_tick().await;
    }
    rt.cache().flush().await.unwrap();

    // the claim_timeout invariant: each branch fired exactly once — one
    // instance and one side effect each, however the tick raced the completion
    assert_eq!(proc.task_by_nid("timeout1").len(), 1);
    assert_eq!(proc.task_by_nid("timeout2").len(), 1);
    assert_eq!(msgs1.data().len(), 1);
    assert_eq!(msgs2.data().len(), 1);

    // the racing task settled into exactly one state — and the store agrees
    let step1 = proc.task_by_nid("step1").first().unwrap().clone();
    assert!(
        step1.state().is_completed(),
        "step1 must have settled, got {:?}",
        step1.state()
    );
    let store = rt.cache().store();
    let row = store
        .tasks()
        .find(&utils::Id::new(&pid, &step1.id).id())
        .await
        .unwrap();
    assert_eq!(
        TaskState::from(row.state.as_str()),
        step1.state(),
        "the settled state must be the one durable state"
    );

    // durable claim markers: exactly the branches that fired, no tearing
    let vars_row = store
        .vars()
        .find(&utils::Id::new(&pid, &step1.id).id())
        .await
        .unwrap();
    let data: Vars = serde_json::from_str(&vars_row.data).unwrap();
    let mut fired = data.get::<Vec<String>>(TASK_TIMEOUTS).unwrap_or_default();
    fired.sort();
    assert_eq!(
        fired,
        vec!["timeout1".to_string(), "timeout2".to_string()],
        "both branches must be claimed exactly once, durably"
    );

    // no duplicate task rows for the process
    let q = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.to_string())));
    let mut rows = store.tasks().query(&q).await.unwrap().rows;
    rows.sort_by(|a, b| a.tid.cmp(&b.tid));
    assert!(
        !rows.windows(2).any(|w| w[0].tid == w[1].tid),
        "duplicate task rows: {:?}",
        rows.iter().map(|r| r.tid.as_str()).collect::<Vec<_>>()
    );

    // nothing is stuck: completing step2's act finishes the process cleanly
    let (_, act2_tid) = r2tx.recv().await;
    rt.do_action(&Action::new(
        &pid,
        &act2_tid,
        EventAction::Next,
        Vars::new(),
    ))
    .await
    .unwrap();
    wait_until("the process finished", || proc.state().is_biz_success()).await;

    engine.close().await;
}

/// Wait until the timeout branches dispatched by `do_tick` have run: they
/// execute on the scheduler lanes, asynchronously from the tick.
async fn wait_for_settled(proc: &Arc<Process>) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let settled = ["timeout1", "timeout2"].iter().all(|nid| {
            let tasks = proc.task_by_nid(nid);
            !tasks.is_empty() && tasks.iter().all(|t| t.state().is_completed())
        });
        if settled {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the dispatched timeout branches did not settle"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// A tick used to fire one timeout child per tick, forever: the branch has no
/// guard and the timed-out task stays in flight. `do_tick` must dispatch each
/// branch once per task instance — the report's "one child per timeout node" —
/// so neither repeated ticks nor a restored process fires the branch again.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_timeout_fires_each_branch_once_across_ticks_and_restart() {
    bounded(
        "sch_step_timeout_fires_each_branch_once_across_ticks_and_restart",
        sch_step_timeout_fires_each_branch_once_across_ticks_and_restart_inner(),
    )
    .await;
}

async fn sch_step_timeout_fires_each_branch_once_across_ticks_and_restart_inner() {
    // one shared store across both engines: the second one restarts against
    // what the first one persisted
    let kv: Arc<dyn KvStore> = Arc::new(MemoryStore::new());
    let engine = Engine::builder()
        .set_store(kv.clone())
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();

    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_timeout(|timeout| timeout.with_id("timeout1"))
            .with_timeout(|timeout| timeout.with_id("timeout2"))
    });
    let pid = utils::longid();
    let proc = rt.create_proc(&pid, &workflow);
    let step1 = proc.tree().node("step1").unwrap();
    let task = proc.create_task(&step1, None).unwrap();
    task.set_state(TaskState::Running);
    rt.cache().store().upsert_proc(&proc).await.unwrap();
    rt.cache().store().persist_task_rows(&task).await.unwrap();

    // five ticks of one still-running step, all landing while the first branch
    // is still queued: it is dispatched once and the following ticks create
    // nothing — neither for the branch that fired nor for the one in flight
    for _ in 0..5 {
        proc.do_tick().await;
    }
    wait_for_settled(&proc).await;

    // ticks after every branch fired create nothing
    for _ in 0..3 {
        proc.do_tick().await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(proc.task_by_nid("timeout1").len(), 1);
    assert_eq!(proc.task_by_nid("timeout2").len(), 1);

    // The marker is durable state of the timed-out task: a process restored
    // from the store does not fire a branch that already fired — even though
    // its step is still in flight and its branches are terminal instances the
    // tick would otherwise re-create.
    //
    // The reload runs on a bare runtime over the same store, not on a second
    // started engine: a started engine's first sweep deletes the rows of a
    // finished process, and a reload racing that sweep reads the proc row but
    // not its task rows (`load_proc` reads them separately) — the reload would
    // then assert against an empty process.
    engine.close().await;
    let rt2 = Runtime::new(&Config::default(), Some(kv.clone())).unwrap();
    let restored = rt2
        .cache()
        .store()
        .load_proc(&pid, &rt2)
        .await
        .unwrap()
        .unwrap();

    // put the timed-out step back in flight: this is the state a crash leaves
    // it in (its branches already ran, the step never completed), and the only
    // thing that can keep the tick from firing them again is the restored
    // marker
    let step = restored.task_by_nid("step1").first().unwrap().clone();
    step.set_state(TaskState::Running);
    for _ in 0..3 {
        restored.do_tick().await;
    }
    assert_eq!(restored.task_by_nid("timeout1").len(), 1);
    assert_eq!(restored.task_by_nid("timeout2").len(), 1);
}

/// The duplicate the tick used to produce was an external side effect, not just
/// an extra task row: with a branch guarded by an always-true condition and
/// `uses`, the same message is sent once — not once per tick.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_timeout_does_not_repeat_a_fired_branch() {
    bounded(
        "sch_step_timeout_does_not_repeat_a_fired_branch",
        sch_step_timeout_does_not_repeat_a_fired_branch_inner(),
    )
    .await;
}

async fn sch_step_timeout_does_not_repeat_a_fired_branch_inner() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_timeout(|timeout| {
            timeout
                .with_id("timeout1")
                .with_if("$cost() >= 0")
                .with_uses(USES_MSG, Vars::new().with("key", "msg1"))
        })
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let sent = engine.signal(Vec::<Message>::default());
    let collector = sent.clone();
    engine.channel().on_message(move |e| {
        let collector = collector.clone();
        async move {
            if e.is_params_key("msg1") {
                collector.update(|data| data.push(e.inner().clone()));
            }
        }
    });

    let step1 = proc.tree().node("step1").unwrap();
    let task = proc.create_task(&step1, None).unwrap();
    task.set_state(TaskState::Running);
    for _ in 0..5 {
        proc.do_tick().await;
    }

    // let the dispatched branch (and any pre-fix duplicate) reach the channel
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(proc.task_by_nid("timeout1").len(), 1);
    assert_eq!(
        sent.data().len(),
        1,
        "a fired timeout branch must not be dispatched again on later ticks"
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_proc_do_tick_skips_terminal_states() {
    bounded(
        "sch_proc_do_tick_skips_terminal_states",
        sch_proc_do_tick_skips_terminal_states_inner(),
    )
    .await;
}

async fn sch_proc_do_tick_skips_terminal_states_inner() {
    // `do_tick` must only fire timeouts for tasks still in flight: a
    // completed / error / aborted / skipped step task must not schedule its
    // timeout children on any tick
    let workflow = Workflow::new().with_id("w1").with_step(|step| {
        step.with_id("s1")
            .with_timeout(|timeout| timeout.with_id("t1"))
    });

    for state in [
        TaskState::Completed,
        TaskState::Error,
        TaskState::Aborted,
        TaskState::Skipped,
    ] {
        let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
        let s1 = proc.tree().node("s1").unwrap();
        let task = proc.create_task(&s1, None).unwrap();
        task.set_state(state.clone());
        proc.do_tick().await;
        assert!(
            proc.task_by_nid("t1").is_empty(),
            "a {state} task must not fire its timeouts"
        );
        drop(engine);
    }

    // control: a running task still fires its timeouts
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let s1 = proc.tree().node("s1").unwrap();
    let task = proc.create_task(&s1, None).unwrap();
    task.set_state(TaskState::Running);
    proc.do_tick().await;
    assert_eq!(proc.task_by_nid("t1").len(), 1);
    drop(engine);
}
