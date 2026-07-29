use crate::event::EventAction;
use crate::utils::test::auto_complete;
use crate::{
    Executor, MessageState, Vars, Workflow,
    scheduler::TaskState,
    utils::{
        self, consts,
        test::{USES_IRQ, USES_SUBFLOW, create_proc},
    },
};

use serde_json::json;

use serial_test::serial;
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_subflow_start() {
    let main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1").with_uses(
            USES_SUBFLOW,
            Vars::from(json!({
            "to": "w2"
            })),
        )
    });

    let w2 = Workflow::new()
        .with_id("w2")
        .with_step(|step| step.with_id("step1"));
    main.print();
    let (engine, proc) = create_proc(&main, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();

    // deploy w2 workflow
    Executor::new(&rt).model().deploy(&w2, None).unwrap();
    channel.on_start(move |e| {
        if e.mid == "w2" {
            rx.update(|data| *data = true);
        }
    });

    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert!(ret)
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_subflow_not_found_error() {
    let main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_SUBFLOW, Vars::from(json!({})))
    });

    main.print();
    let (engine, proc) = create_proc(&main, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);

    rt.launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert!(proc.state().is_error())
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_subflow_act_running() {
    let main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1").with_uses(
            USES_SUBFLOW,
            Vars::from(json!({
                "to": "w2",
            })),
        )
    });

    let w2 = Workflow::new().with_id("w2").with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });

    main.print();
    let (engine, proc) = create_proc(&main, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);

    Executor::new(&rt).model().deploy(&w2, None).unwrap();
    let channel = engine.channel();
    channel.on_message(move |e| {
        println!("message: {:?}", e.inner());
    });

    channel.on_start(move |e| {
        if e.mid == "w2" {
            rx.close();
        }
    });

    rt.launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Running
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_subflow_act_complete() {
    let main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1").with_uses(
            USES_SUBFLOW,
            Vars::from(json!({
                "to": "w2",
            })),
        )
    });

    let w2 = Workflow::new().with_id("w2").with_step(|step| {
        step.with_id("s1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });

    main.print();
    let main_pid = utils::longid();
    let (engine, proc) = create_proc(&main, &main_pid);
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();

    let channel = engine.channel();
    channel.on_complete(move |e| {
        if e.mid == "main" {
            rx.close();
        }
    });
    Executor::new(&rt).model().deploy(&w2, None).unwrap();
    channel.on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            rt.do_action2(&e.pid, &e.tid, EventAction::Next, Vars::new())
                .unwrap();
        }
    });

    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Completed
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_subflow_act_skip() {
    let main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1").with_uses(
            USES_SUBFLOW,
            Vars::from(json!({
                "to":"w2",
            })),
        )
    });

    let w2 = Workflow::new().with_id("w2").with_step(|step| {
        step.with_id("s1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });

    main.print();
    let main_pid = utils::longid();
    let (engine, proc) = create_proc(&main, &main_pid);
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    let channel = engine.channel();
    channel.on_complete(move |e| {
        if e.mid == "main" {
            rx.close();
        }
    });
    Executor::new(&rt).model().deploy(&w2, None).unwrap();
    channel.on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            rt.do_action2(&e.pid, &e.tid, EventAction::Skip, Vars::new())
                .unwrap();
        }
    });

    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;
    proc.print();

    // sub workflow's skip does not affect the parent act state
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Completed
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_subflow_act_abort() {
    let main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1").with_uses(
            USES_SUBFLOW,
            Vars::from(json!({
                     "to": "w2",
            })),
        )
    });

    let w2 = Workflow::new().with_id("w2").with_step(|step| {
        step.with_id("s1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });

    main.print();
    let main_pid = utils::longid();
    let (engine, proc) = create_proc(&main, &main_pid);
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    // auto_complete(&engine, &rx);
    let channel = engine.channel();

    Executor::new(&rt).model().deploy(&w2, None).unwrap();
    channel.on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            rt.do_action2(&e.pid, &e.tid, EventAction::Abort, Vars::new())
                .unwrap();
        }
    });
    channel.on_complete(move |e| {
        if e.mid == "main" {
            rx.close();
        }
    });
    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;
    proc.print();

    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Aborted
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_subflow_act_error() {
    let main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1").with_uses(
            USES_SUBFLOW,
            Vars::from(json!({
                    "to": "w2",
            })),
        )
    });

    let w2 = Workflow::new().with_id("w2").with_step(|step| {
        step.with_id("s1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });

    main.print();
    let main_pid = utils::longid();
    let (engine, proc) = create_proc(&main, &main_pid);
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    let channel = engine.channel();

    Executor::new(&rt).model().deploy(&w2, None).unwrap();
    channel.on_error(move |e| {
        if e.mid == "main" {
            rx.close();
        }
    });
    channel.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.set(consts::ACT_ERR_CODE, "err1");
            options.set(consts::ACT_ERR_MESSAGE, "sub workflow error");
            rt.do_action2(&e.pid, &e.tid, EventAction::Error, options)
                .unwrap();
        }
    });

    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert!(
        proc.task_by_nid("step1")
            .first()
            .unwrap()
            .state()
            .is_error()
    );
}
