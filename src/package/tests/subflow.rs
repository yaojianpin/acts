use crate::event::EventAction;
use crate::{
    Act, Executor, MessageState, Vars, Workflow,
    scheduler::TaskState,
    utils::{self, consts, test::create_proc_signal},
};

use serde_json::json;

#[tokio::test]
async fn pack_subflow_start() {
    let mut main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1").with_act(Act::subflow(json!({
        "to": "w2"
        })))
    });

    let w2 = Workflow::new()
        .with_id("w2")
        .with_step(|step| step.with_id("step1"));
    main.print();
    let (proc, scher, emitter, tx, rx) = create_proc_signal(&mut main, &utils::longid());

    // deploy w2 workflow
    Executor::new(&scher).model().deploy(&w2).unwrap();
    emitter.on_start(move |e| {
        if e.model.id == "w2" {
            rx.update(|data| *data = true);
        }
    });

    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert!(ret)
}

#[tokio::test]
async fn pack_subflow_not_found_error() {
    let mut main = Workflow::new()
        .with_id("main")
        .with_step(|step| step.with_id("step1").with_act(Act::subflow(json!({}))));

    main.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut main, &utils::longid());
    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert!(proc.state().is_error())
}

#[tokio::test]
async fn pack_subflow_act_running() {
    let mut main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1").with_act(
            Act::subflow(json!({
            "to": "w2",
            }))
            .with_id("call1"),
        )
    });

    let w2 = Workflow::new().with_id("w2").with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")).with_id("act1"))
    });

    main.print();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<String>(&mut main, &utils::longid());
    Executor::new(&scher).model().deploy(&w2).unwrap();
    emitter.on_message(move |e| {
        println!("message: {:?}", e.inner());
    });

    emitter.on_start(move |e| {
        if e.model.id == "w2" {
            rx.close();
        }
    });

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("call1").first().unwrap().state(),
        TaskState::Running
    );
}

#[tokio::test]
async fn pack_subflow_act_complete() {
    let mut main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1").with_act(
            Act::subflow(json!({
            "to": "w2",
            }))
            .with_id("call1"),
        )
    });

    let w2 = Workflow::new().with_id("w2").with_step(|step| {
        step.with_id("s1")
            .with_act(Act::irq(|act| act.with_key("act1")).with_id("act1"))
    });

    main.print();
    let main_pid = utils::longid();
    let (proc, scher, emitter, tx, ..) = create_proc_signal::<()>(&mut main, &main_pid);

    Executor::new(&scher).model().deploy(&w2).unwrap();
    emitter.on_message(move |e| {
        if e.is_key("act1") && e.is_state(MessageState::Created) {
            let options = Vars::new();
            e.do_action(&e.pid, &e.tid, EventAction::Next, &options)
                .unwrap();
        }
    });

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("call1").first().unwrap().state(),
        TaskState::Completed
    );
}

#[tokio::test]
async fn pack_subflow_act_skip() {
    let mut main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1").with_act(
            Act::subflow(json!({
            "to":"w2",
            }))
            .with_id("call1"),
        )
    });

    let w2 = Workflow::new().with_id("w2").with_step(|step| {
        step.with_id("s1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    main.print();
    let main_pid = utils::longid();
    let (proc, scher, emitter, tx, ..) = create_proc_signal::<()>(&mut main, &main_pid);

    Executor::new(&scher).model().deploy(&w2).unwrap();
    emitter.on_message(move |e| {
        if e.is_key("act1") && e.is_state(MessageState::Created) {
            let options = Vars::new();
            e.do_action(&e.pid, &e.tid, EventAction::Skip, &options)
                .unwrap();
        }
    });

    scher.launch(&proc);
    tx.recv().await;
    proc.print();

    // sub workflow's skip does not affect the parent act state
    assert_eq!(
        proc.task_by_nid("call1").first().unwrap().state(),
        TaskState::Completed
    );
}

#[tokio::test]
async fn pack_subflow_act_abort() {
    let mut main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1").with_act(
            Act::subflow(json!({
                 "to": "w2",
            }))
            .with_id("call1"),
        )
    });

    let w2 = Workflow::new().with_id("w2").with_step(|step| {
        step.with_id("s1")
            .with_act(Act::irq(|act| act.with_key("act1")).with_id("act1"))
    });

    main.print();
    let main_pid = utils::longid();
    let (proc, scher, emitter, tx, _) = create_proc_signal::<()>(&mut main, &main_pid);

    Executor::new(&scher).model().deploy(&w2).unwrap();
    emitter.on_message(move |e| {
        println!("message: {:?}", e.inner());
        if e.is_key("act1") && e.is_state(MessageState::Created) {
            let options = Vars::new();
            e.do_action(&e.pid, &e.tid, EventAction::Abort, &options)
                .unwrap();
        }
    });
    scher.launch(&proc);
    tx.recv().await;
    proc.print();

    assert_eq!(
        proc.task_by_nid("call1").first().unwrap().state(),
        TaskState::Aborted
    );
}

#[tokio::test]
async fn pack_subflow_act_error() {
    let mut main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1").with_act(
            Act::subflow(json!({
                "to": "w2",
            }))
            .with_id("call1"),
        )
    });

    let w2 = Workflow::new().with_id("w2").with_step(|step| {
        step.with_id("s1")
            .with_act(Act::irq(|act| act.with_key("act1")).with_id("act1"))
    });

    main.print();
    let main_pid = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<()>(&mut main, &main_pid);

    Executor::new(&scher).model().deploy(&w2).unwrap();
    emitter.on_error(move |e| {
        if e.model.id == "main" {
            rx.close();
        }
    });
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.set(consts::ACT_ERR_CODE, "err1");
            options.set(consts::ACT_ERR_MESSAGE, "sub workflow error");
            e.do_action(&e.pid, &e.tid, EventAction::Error, &options)
                .unwrap();
        }
    });

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert!(
        proc.task_by_nid("call1")
            .first()
            .unwrap()
            .state()
            .is_error()
    );
}
