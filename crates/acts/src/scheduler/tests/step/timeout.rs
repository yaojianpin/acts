use crate::{
    Config, Engine, Message, Vars, Workflow,
    scheduler::{Process, Runtime, TaskState},
    store::{KvStore, MemoryStore},
    utils::{
        self,
        test::{USES_IRQ, USES_MSG, auto_complete, create_proc},
    },
};
use std::{sync::Arc, time::Duration};

use serial_test::serial;

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

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_timeout_one() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_timeout(|step| {
                step.with_id("timout1")
                    .with_if(r#"$cost() >= 1000"#)
                    .with_uses(USES_MSG, Vars::new().with("key", "msg1"))
            })
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();

    channel.on_message(move |e| {
        let rx = rx.clone();
        async move {
            if e.is_params_key("msg1") {
                rx.send(true);
            }
        }
    });

    engine.runtime().launch(&proc).await.unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert!(ret)
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_timeout_many() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_timeout(|step| {
                step.with_id("timeout1")
                    .with_if(r#"$cost() >= 1000 && $cost() < 2000"#)
                    .with_uses(USES_MSG, Vars::new().with("key", "msg1"))
            })
            .with_timeout(|step| {
                step.with_id("timeout2")
                    .with_if(r#"$cost() >= 2000 && $cost() < 3000"#)
                    .with_uses(USES_MSG, Vars::new().with("key", "msg2"))
            })
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let (tx, rx) = engine.signal(Vec::<Message>::default()).double();
    let channel = engine.channel();
    channel.on_message(move |e| {
        let rx = rx.clone();
        async move {
            // println!("message: {e:?}");
            if e.is_params_key("msg1") {
                rx.update(|data| data.push(e.inner().clone()));
            }

            if e.is_params_key("msg2") {
                rx.update(|data| data.push(e.inner().clone()));
                rx.close();
            }
        }
    });

    engine.runtime().launch(&proc).await.unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.len(), 2)
}

/// A tick used to fire one timeout child per tick, forever: the branch has no
/// guard and the timed-out task stays in flight. `do_tick` must dispatch each
/// branch once per task instance — the report's "one child per timeout node" —
/// so neither repeated ticks nor a restored process fires the branch again.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_timeout_fires_each_branch_once_across_ticks_and_restart() {
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
