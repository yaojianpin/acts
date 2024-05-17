use crate::{
    sch::{tests::create_proc_signal, TaskState},
    utils::{self, consts},
    Act, Error, Manager, Vars, Workflow,
};
use serde_json::json;

#[tokio::test]
async fn sch_act_call_start() {
    let mut main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1")
            .with_act(Act::call(|act| act.with_mid("w2")))
    });

    let w2 = Workflow::new()
        .with_id("w2")
        .with_step(|step| step.with_id("step1"));

    main.print();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<bool>(&mut main, &utils::longid());

    // deploy w2 workflow
    Manager::new(&scher).deploy(&w2).unwrap();
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
async fn sch_act_call_not_found_error() {
    let mut main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1")
            .with_act(Act::call(|act| act.with_mid("not_exists")))
    });

    main.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut main, &utils::longid());
    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert!(proc.state().is_error())
}

#[tokio::test]
async fn sch_act_call_act_running() {
    let mut main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1")
            .with_act(Act::call(|act| act.with_id("act1").with_mid("w2")))
    });

    let w2 = Workflow::new().with_id("w2").with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|act| act.with_id("act2")))
    });

    main.print();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<()>(&mut main, &utils::longid());
    Manager::new(&scher).deploy(&w2).unwrap();
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
        proc.task_by_nid("act1").get(0).unwrap().state(),
        TaskState::Running
    );
}

#[tokio::test]
async fn sch_act_call_act_complete() {
    let mut main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1").with_act(Act::call(|act| {
            act.with_id("act1")
                .with_mid("w2")
                .with_input("pid", json!("sub2"))
        }))
    });

    let w2 = Workflow::new().with_id("w2").with_step(|step| {
        step.with_id("s1")
            .with_act(Act::req(|act| act.with_id("act2")))
    });

    main.print();
    let main_pid = utils::longid();
    let (proc, scher, emitter, tx, _) = create_proc_signal::<()>(&mut main, &main_pid);

    Manager::new(&scher).deploy(&w2).unwrap();
    emitter.on_message(move |e| {
        if e.is_key("act2") && e.is_state("created") {
            let options = Vars::new();
            e.do_action(&e.pid, &e.tid, consts::EVT_NEXT, &options)
                .unwrap();
        }
    });

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").get(0).unwrap().state(),
        TaskState::Completed
    );
}

#[tokio::test]
async fn sch_act_call_act_skip() {
    let mut main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1").with_act(Act::call(|act| {
            act.with_id("act1")
                .with_mid("w2")
                .with_input("pid", json!("sub1"))
        }))
    });

    let w2 = Workflow::new().with_id("w2").with_step(|step| {
        step.with_id("s1")
            .with_act(Act::req(|act| act.with_id("act2")))
    });

    main.print();
    let main_pid = utils::longid();
    let (proc, scher, emitter, tx, _) = create_proc_signal::<()>(&mut main, &main_pid);

    Manager::new(&scher).deploy(&w2).unwrap();
    emitter.on_message(move |e| {
        if e.is_key("act2") && e.is_state("created") {
            let options = Vars::new();
            e.do_action(&e.pid, &e.tid, consts::EVT_SKIP, &options)
                .unwrap();
        }
    });

    scher.launch(&proc);
    tx.recv().await;
    proc.print();

    // sub workflow's skip does not affect the parent act state
    assert_eq!(
        proc.task_by_nid("act1").get(0).unwrap().state(),
        TaskState::Completed
    );
}

#[tokio::test]
async fn sch_act_call_act_abort() {
    let mut main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1").with_act(Act::call(|act| {
            act.with_id("act1")
                .with_mid("w2")
                .with_input("pid", json!("sub1"))
        }))
    });

    let w2 = Workflow::new().with_id("w2").with_step(|step| {
        step.with_id("s1")
            .with_act(Act::req(|act| act.with_id("act2")))
    });

    main.print();
    let main_pid = utils::longid();
    let (proc, scher, emitter, tx, _) = create_proc_signal::<()>(&mut main, &main_pid);

    Manager::new(&scher).deploy(&w2).unwrap();
    emitter.on_message(move |e| {
        println!("message: {:?}", e.inner());
        if e.is_key("act2") && e.is_state("created") {
            let options = Vars::new();
            e.do_action(&e.pid, &e.tid, consts::EVT_ABORT, &options)
                .unwrap();
        }
    });
    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    
    assert_eq!(
        proc.task_by_nid("act1").get(0).unwrap().state(),
        TaskState::Aborted
    );
}

#[tokio::test]
async fn sch_act_call_act_error() {
    let mut main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1").with_act(Act::call(|act| {
            act.with_id("act1")
                .with_mid("w2")
                .with_input("pid", json!("sub1"))
        }))
    });

    let w2 = Workflow::new().with_id("w2").with_step(|step| {
        step.with_id("s1")
            .with_act(Act::req(|act| act.with_id("act2")))
    });

    main.print();
    let main_pid = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<()>(&mut main, &main_pid);

    Manager::new(&scher).deploy(&w2).unwrap();
    emitter.on_error(move |e| {
        if e.model.id == "main" {
            rx.close();
        }
    });
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act2") && e.is_state("created") {
            let mut options = Vars::new();
            options.set(
                consts::ACT_ERR_KEY,
                Error::new("sub workflow error", "err1"),
            );
            e.do_action(&e.pid, &e.tid, consts::EVT_ERR, &options)
                .unwrap();
        }
    });

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert!(proc.task_by_nid("act1").get(0).unwrap().state().is_error(),);
}
