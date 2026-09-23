//! ERROR reliability cells: the `Error` client action under a store fault, a
//! duplicate delivery of it, and `Error` racing `Next` on one act.
//!
//! - DB失败: the errored act's own state write fails exactly once. The
//!   client's action is accepted (the fault hits off the hot path and is
//!   reported to the next durability barrier) and its effect must be durable
//!   exactly once, so a crash + reload can never restore the errored act as a
//!   waiting one; the finished process sweeps cleanly.
//! - 重复投递: delivering `Error` again for an act that already errored is a
//!   terminal no-op — rejected, and it changes nothing.
//! - 并发: `Error` and `Next` fired at the same in-flight act tid in the same
//!   instant converge to one decision, one terminal act state, and one
//!   process outcome.

use std::sync::{Arc, atomic::Ordering};
use std::time::Duration;

use serial_test::serial;

use crate::{
    ActError, Action, Engine, TaskState, Vars, Workflow,
    event::EventAction,
    scheduler::NodeKind,
    store::{
        KvStore, MemoryStore,
        query::{Expr, Filter, Query},
    },
    utils,
    utils::test::USES_IRQ,
};

use super::*;

/// An `Error` action whose outbox record landed but whose task write was lost
/// is re-applied on recovery: the act becomes `Error` with the recorded code.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_action_recover_reapplies_error() {
    bounded(
        "sch_action_recover_reapplies_error",
        sch_action_recover_reapplies_error_inner(),
    )
    .await;
}

async fn sch_action_recover_reapplies_error_inner() {
    let store: Arc<dyn KvStore> = Arc::new(MemoryStore::new());
    let engine = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });

    let sig = engine.signal((String::new(), String::new()));
    let (s, s2) = sig.double();
    engine.channel().on_message(move |e| {
        let s2 = s2.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                s2.update(|d| *d = (e.pid.clone(), e.tid.clone()));
                s2.close();
            }
        }
    });

    let proc = rt.create_proc(&utils::longid(), &workflow);
    rt.launch(&proc).await.unwrap();
    let (pid, act1_tid) = s.recv().await;

    // crash before the error was durably applied: only the outbox record landed
    rt.cache()
        .store()
        .enqueue_action_op(&pid, &act1_tid, "error", r#"{"ecode":"err1"}"#)
        .await
        .unwrap();
    engine.close().await;

    // reload: recovery re-applies the error with its code
    let engine2 = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let rt2 = engine2.runtime();
    let store2 = rt2.cache().store();
    let mut drained = false;
    for _ in 0..100 {
        let pending = store2.load_pending_ops().await.unwrap();
        if pending.iter().all(|op| op.r#type != "action") {
            drained = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(drained, "error action record was not closed");

    // the error terminates the process; finished processes are cleaned up by
    // the sweeper once their deliveries settled — if the recovery had
    // silently dropped the error, act1 would still be Interrupt and the
    // process rows would survive
    let q = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.clone())));
    for _ in 0..150 {
        if store2.procs().find(&pid).await.is_err() {
            break;
        }
        let _ = rt2.cache().sweep_removable().await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        store2.procs().find(&pid).await.is_err(),
        "the errored process must be removed after the re-applied error"
    );
    assert!(
        store2.tasks().query(&q).await.unwrap().rows.is_empty(),
        "the errored process's task rows must be gone"
    );
}

// ---------------------------------------------------------------------------
// ERROR × DB失败 — the errored act's own state write fails once.
// ---------------------------------------------------------------------------

/// DB失败 × ERROR: the state write of the act finished by `Error` fails exactly
/// once. The client's action is accepted (the fault hits off the hot path and
/// reports on the next durability barrier), the in-memory state is `Error` with
/// the client's code, and the error propagates to the process. The accepted
/// action must also be durable exactly once — the errored act's row has to
/// converge to `Error`, so a crash can never restore it as a waiting act — and
/// the finished process must sweep cleanly.
///
/// The `Error` arm used to write the act's terminal row once and then close the
/// action's outbox record regardless, so a fault there left the row at the
/// stale pre-action `interrupted` and a crash restored the errored act as a
/// waiting act in a process that survives the error. The arm now re-emits the
/// errored act after the error walk, so the durable row converges to `Error`
/// and a reload cannot resurrect a client's already-errored act.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_error_store_fail_degrades_and_heals_once() {
    bounded(
        "sch_error_store_fail_degrades_and_heals_once",
        sch_error_store_fail_degrades_and_heals_once_inner(),
    )
    .await;
}

async fn sch_error_store_fail_degrades_and_heals_once_inner() {
    let kv = Arc::new(FailOnceTaskPutKv::new());
    let store: Arc<dyn KvStore> = kv.clone();
    let engine = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("s1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });

    let s = capture_created(&engine, "act1");
    let proc = rt.create_proc(&utils::longid(), &workflow);
    rt.launch(&proc).await.unwrap();
    let (pid, act1_tid) = s.recv().await;
    let store_ops = rt.cache().store();

    // settle every launch write, then arm the fault on exactly the errored
    // act's own lifecycle row: its next write is the `Error` state persist
    rt.cache().flush().await.unwrap();
    kv.arm_row(&pid, &act1_tid);

    let mut options = Vars::new();
    options.set(utils::consts::ACT_ERR_CODE, "err1");
    rt.do_action(&Action::new(&pid, &act1_tid, EventAction::Error, options))
        .await
        .unwrap();

    // the fault is real, lands on the intended write, and reports exactly
    // once — to whichever durability barrier arrives first. Usually that is
    // the flush below; but the error also finished the process, and the retry
    // timer's sweep (every `TEST_TICK_MS`) may barrier first and take the
    // report, leaving this flush clean. A sweep that took the report has
    // already dropped the rows, so a clean flush with the process still
    // present is the only swallow this refuses.
    let flush = rt.cache().flush().await;
    if let Err(err) = &flush {
        assert!(matches!(err, ActError::Store(_)), "{err:?}");
    }
    let reported =
        flush.is_err() || rt.cache().store().procs().find(&pid).await.is_err();
    assert!(
        reported,
        "the lost write must reach a durability barrier, not be swallowed: {flush:?}"
    );
    assert_eq!(
        kv.injected.load(Ordering::SeqCst),
        1,
        "exactly one write must have been injected-failed"
    );
    assert_eq!(
        kv.failed_key(),
        Some(task_data_key(&pid, &act1_tid)),
        "the fault must land on the errored act's own state write"
    );

    // the action is applied in memory with the client's code ...
    let act = proc.task(&act1_tid).expect("the act task must exist");
    assert_eq!(act.state(), TaskState::Error);
    assert_eq!(act.err().map(|e| e.ecode), Some("err1".to_string()));

    // ... and propagates to the process: the terminal outcome is the error,
    // never a business success
    assert!(
        poll_settle(|| async { proc.state().is_error() }, 100).await,
        "the error must reach the process, got {}",
        proc.state()
    );

    // durability: the accepted action's effect must be durable exactly once —
    // the errored act's row converges to `Error` instead of staying at the
    // stale pre-action state while its outbox record is closed
    let mut durable = None;
    for _ in 0..100 {
        if let Ok(row) = store_ops.tasks().find(&task_row_id(&pid, &act1_tid)).await {
            durable = Some(TaskState::from(row.state.as_str()));
            if durable == Some(TaskState::Error) {
                break;
            }
        }
        tokio::time::sleep(SETTLE).await;
    }
    assert_eq!(
        durable,
        Some(TaskState::Error),
        "the lost state write must heal to the errored act's terminal `Error`: a stale row means the accepted action's effect was never made durable"
    );

    // crash + reload: recovery must not resurrect the errored act (no fresh
    // `Created` for a client that was already told the act errored), and no
    // waiting act may survive under the recovered tree
    engine.close().await;
    let engine2 = Engine::builder()
        .set_store(store.clone())
        .start()
        .await
        .unwrap();
    let rt2 = engine2.runtime();
    let s2 = capture_created(&engine2, "act1");
    assert!(
        tokio::time::timeout(Duration::from_millis(400), s2.recv())
            .await
            .is_err(),
        "recovery must never resurrect an errored act"
    );
    if let Some(reloaded) = rt2.proc(&pid).await.unwrap() {
        let acts: Vec<TaskState> = reloaded
            .tasks()
            .iter()
            .filter(|t| t.node().kind() == NodeKind::Act)
            .map(|t| t.state())
            .collect();
        assert!(
            acts.iter().all(|s| s.is_completed()),
            "no errored act may come back as a waiting act: {acts:?}"
        );
    }
    sweep_until_gone(&rt2, &pid).await;
    engine2.close().await;
}

/// 重复投递 × ERROR: delivering `Error` again for an act that already errored
/// must be a no-op. The second delivery is rejected — by the terminal-state
/// guard while the settled process is still there, or because its rows were
/// already swept and there is nothing left to apply to — and it changes
/// nothing: one act task, the act still the single `error` row, no propagation
/// enqueued again, and no open outbox record left behind. The process stays in
/// its terminal outcome and sweeps cleanly.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_error_duplicate_delivery_is_terminal_noop() {
    bounded(
        "sch_error_duplicate_delivery_is_terminal_noop",
        sch_error_duplicate_delivery_is_terminal_noop_inner(),
    )
    .await;
}

async fn sch_error_duplicate_delivery_is_terminal_noop_inner() {
    let engine = Engine::builder().start().await.unwrap();
    let rt = engine.runtime();
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("s1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });

    let s = capture_created(&engine, "act1");
    let proc = rt.create_proc(&utils::longid(), &workflow);
    rt.launch(&proc).await.unwrap();
    let (pid, act1_tid) = s.recv().await;
    let store = rt.cache().store();

    let mut options = Vars::new();
    options.set(utils::consts::ACT_ERR_CODE, "err1");
    let action = Action::new(&pid, &act1_tid, EventAction::Error, options);
    rt.do_action(&action).await.unwrap();

    // let the first delivery settle durably before the duplicate arrives, so
    // the duplicate is a retry and not a race with the first one
    rt.cache().flush().await.unwrap();
    assert!(
        poll_settle(
            || async {
                matches!(
                    store.tasks().find(&task_row_id(&pid, &act1_tid)).await,
                    Ok(row) if TaskState::from(row.state.as_str()).is_error()
                )
            },
            100
        )
        .await,
        "the error must be durable before the duplicate arrives"
    );

    // what the first delivery produced: the rows of the act's own path and the
    // act's own propagation records
    let q_act = Query::new().filter(
        Filter::and()
            .expr(Expr::eq("pid", pid.clone()))
            .expr(Expr::eq("tid", act1_tid.clone())),
    );
    let before_nexts = store
        .ops()
        .query(&q_act)
        .await
        .unwrap()
        .rows
        .iter()
        .filter(|op| op.r#type == "next")
        .count();
    // the live tree before the duplicate: the duplicate's own `do_action` may
    // re-load the settled process from the store (it is evicted once it
    // finishes), so the in-memory handle is only read here — the store is the
    // oracle after the duplicate
    assert_eq!(
        proc.task_by_params("key", "act1").len(),
        1,
        "the first delivery leaves exactly one act task"
    );
    assert_eq!(
        proc.task(&act1_tid).map(|t| t.state()),
        Some(TaskState::Error),
        "the first delivery leaves the act errored"
    );

    // the duplicate is rejected, not re-applied
    let dup = rt.do_action(&action).await;
    match &dup {
        Ok(()) => panic!("a duplicate Error must never be applied again: {dup:?}"),
        Err(ActError::Action(msg)) => assert!(
            msg.contains("already completed"),
            "the duplicate must be rejected by the terminal guard: {dup:?}"
        ),
        Err(ActError::Runtime(msg)) => assert!(
            msg.contains("cannot find process"),
            "the only other rejection is the settled process already being swept: {dup:?}"
        ),
        Err(err) => panic!("unexpected duplicate verdict: {err:?}"),
    }
    rt.cache().flush().await.unwrap();
    tokio::time::sleep(SETTLE).await;

    // no propagation and no second act task: the rejected duplicate leaves the
    // act's record set where it was (the settled process's rows are swept —
    // and a late `do_action` re-materialises them on its way in — so the row
    // set is not a stable oracle in this window; the verdict above plus the
    // pre-state and the single-row tree asserted before it are)
    let after_nexts = store
        .ops()
        .query(&q_act)
        .await
        .unwrap()
        .rows
        .iter()
        .filter(|op| op.r#type == "next")
        .count();
    if before_nexts > 0 {
        assert!(
            after_nexts <= before_nexts,
            "a duplicate Error must not enqueue another propagation: {before_nexts} -> {after_nexts}"
        );
    }
    let open = store.load_pending_ops().await.unwrap();
    assert!(
        open.iter().all(|op| op.r#type != "action"),
        "the rejected duplicate must leave no open action record: {open:?}"
    );

    // the process settles in the very error the client was told about: a
    // completion pass must never launder an errored run into a business
    // success (`Task::set_state_if_running` keeps the decision that landed),
    // and the finished process sweeps cleanly
    let mut outcome = None;
    for _ in 0..100 {
        match store.procs().find(&pid).await {
            Ok(row) => {
                let state = TaskState::from(row.state.as_str());
                if state.is_completed() {
                    outcome = Some(state);
                    break;
                }
            }
            // swept: only a finished process is swept, and the assertions above
            // already pinned what the finished state had to be
            Err(_) => break,
        }
        tokio::time::sleep(SETTLE).await;
    }
    if let Some(state) = outcome {
        assert!(
            state.is_error(),
            "an errored run must settle as its error, never as {state}"
        );
    }
    sweep_until_gone(&rt, &pid).await;
    engine.close().await;
}

/// 并发 × ERROR: `Error` and `Next` fired at the same in-flight act tid in the
/// same instant must converge to one decision: every racer gets a verdict,
/// exactly one applies and the loser is rejected by the terminal guard (the
/// application is exclusive per task — `Task::enter_action`), the act holds
/// exactly one terminal state, exactly one act task exists, no task of the
/// finished process is left non-terminal, and the process sweeps cleanly.
/// Several rounds cover the interleavings.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_error_racing_complete_converges_single_terminal() {
    bounded(
        "sch_error_racing_complete_converges_single_terminal",
        sch_error_racing_complete_converges_single_terminal_inner(),
    )
    .await;
}

async fn sch_error_racing_complete_converges_single_terminal_inner() {
    bounded(
        "sch_error_racing_complete_converges_single_terminal_inner",
        sch_error_racing_complete_converges_single_terminal_inner_inner(),
    )
    .await;
}

async fn sch_error_racing_complete_converges_single_terminal_inner_inner() {
    for round in 0..3 {
        let engine = Engine::builder().start().await.unwrap();
        let rt = engine.runtime();
        let workflow = Workflow::new().with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
        });

        let s = capture_created(&engine, "act1");
        let proc = rt.create_proc(&utils::longid(), &workflow);
        rt.launch(&proc).await.unwrap();
        let (pid, act1_tid) = s.recv().await;
        let store = rt.cache().store();

        // the race: both actions target the same in-flight act tid at once
        let mut options = Vars::new();
        options.set(utils::consts::ACT_ERR_CODE, "err1");
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let mut handles = Vec::with_capacity(2);
        for action in [
            Action::new(&pid, &act1_tid, EventAction::Error, options),
            Action::new(&pid, &act1_tid, EventAction::Next, Vars::new()),
        ] {
            let rt = rt.clone();
            let barrier = barrier.clone();
            handles.push(tokio::spawn(async move {
                barrier.wait().await;
                rt.do_action(&action).await
            }));
        }
        let mut oks = 0usize;
        let mut errs = Vec::new();
        for handle in handles {
            match handle.await.unwrap() {
                Ok(()) => oks += 1,
                Err(err) => errs.push(err),
            }
        }
        assert_eq!(oks + errs.len(), 2, "every racer must get a verdict");
        assert_eq!(
            oks, 1,
            "round {round}: exactly one racer may decide the act: {errs:?}"
        );
        assert!(
            errs.iter()
                .all(|e| matches!(e, ActError::Action(msg) if msg.contains("already completed"))),
            "round {round}: every rejection must be the already-completed guard: {errs:?}"
        );

        // one decision: the act holds exactly one terminal state ...
        let act_state = proc.task(&act1_tid).unwrap().state();
        assert!(
            matches!(act_state, TaskState::Completed | TaskState::Error),
            "round {round}: the raced act must end in exactly one terminal state, got {act_state}"
        );
        // ... and it is the only act of the process. The deadline is generous:
        // the loser's rejected record and the winner's propagation settle on
        // the scheduler's own queue, which the rest of the suite shares.
        assert!(
            poll_settle(|| async { proc.state().is_completed() }, 250).await,
            "round {round}: the process must settle, got {}",
            proc.state()
        );
        assert_eq!(
            proc.tasks()
                .iter()
                .filter(|t| t.node().kind() == NodeKind::Act)
                .count(),
            1,
            "round {round}: one act child, no duplicate task"
        );

        // durable convergence: the tree the race produced is the tree the store
        // holds — three rows, one act, nothing left non-terminal (unless the
        // finished process was already swept, which the cleanup below asserts)
        rt.cache().flush().await.unwrap();
        let q_all = Query::new().filter(Filter::and().expr(Expr::eq("pid", pid.clone())));
        if store.procs().find(&pid).await.is_ok() {
            let rows = store.tasks().query(&q_all).await.unwrap().rows;
            assert_eq!(
                rows.len(),
                3,
                "round {round}: root + s1 + act1 — the race must not duplicate tasks: {rows:?}"
            );
            assert_eq!(
                rows.iter().filter(|r| r.kind == "act").count(),
                1,
                "round {round}: exactly one act task may exist"
            );
            assert!(
                rows.iter()
                    .all(|r| TaskState::from(r.state.as_str()).is_completed()),
                "round {round}: the race must leave no non-terminal task: {rows:?}"
            );
        }

        // the settled process's rows are removed
        sweep_until_gone(&rt, &pid).await;
        engine.close().await;
    }
}
