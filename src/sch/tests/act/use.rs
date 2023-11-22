use crate::{
    event::Emitter,
    sch::{Proc, Scheduler, TaskState},
    utils::{self, consts},
    Engine, Manager, Vars, Workflow,
};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn sch_act_use_start() {
    let ret = Arc::new(Mutex::new(false));
    let mut main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1")
            .with_act(|act| act.with_id("act1").with_use("w2"))
    });

    let w2 = Workflow::new()
        .with_id("w2")
        .with_step(|step| step.with_id("step1"));

    main.print();
    let (proc, scher, emitter) = create_proc(&mut main, &utils::longid());

    // deploy w2 workflow
    Manager::new(&scher).deploy(&w2).unwrap();
    let r = ret.clone();
    emitter.on_start(move |e| {
        if e.mid == "w2" {
            *r.lock().unwrap() = true;
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert!(*ret.lock().unwrap())
}

#[tokio::test]
async fn sch_act_use_not_found_error() {
    let ret = Arc::new(Mutex::new(false));
    let mut main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1")
            .with_act(|act| act.with_id("act1").with_use("not_exists"))
    });

    main.print();
    let (proc, scher, emitter) = create_proc(&mut main, &utils::longid());
    let r = ret.clone();
    emitter.on_error(move |e| {
        *r.lock().unwrap() = true;
        e.close();
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert!(*ret.lock().unwrap())
}

#[tokio::test]
async fn sch_act_use_act_running() {
    let mut main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1")
            .with_act(|act| act.with_id("act1").with_use("w2"))
    });

    let w2 = Workflow::new()
        .with_id("w2")
        .with_step(|step| step.with_id("step1").with_act(|act| act.with_id("act2")));

    main.print();
    let (proc, scher, emitter) = create_proc(&mut main, &utils::longid());
    Manager::new(&scher).deploy(&w2).unwrap();
    emitter.on_start(move |e| {
        if e.mid == "w2" {
            e.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").get(0).unwrap().state(),
        TaskState::Running
    );
}

#[tokio::test]
async fn sch_act_use_act_complete() {
    let mut main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1").with_act(|act| {
            act.with_id("act1")
                .with_use("w2")
                .with_input("pid", json!("sub1"))
        })
    });

    let w2 = Workflow::new()
        .with_id("w2")
        .with_step(|step| step.with_id("s1").with_act(|act| act.with_id("act2")));

    main.print();
    let main_pid = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut main, &main_pid);

    Manager::new(&scher).deploy(&w2).unwrap();
    emitter.on_message(move |e| {
        if e.is_key("act2") && e.is_state("created") {
            let options = Vars::new();
            e.do_action(&e.proc_id, &e.id, consts::EVT_COMPLETE, &options)
                .unwrap();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").get(0).unwrap().state(),
        TaskState::Success
    );
}

#[tokio::test]
async fn sch_act_use_act_skip() {
    let mut main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1").with_act(|act| {
            act.with_id("act1")
                .with_use("w2")
                .with_input("pid", json!("sub1"))
        })
    });

    let w2 = Workflow::new()
        .with_id("w2")
        .with_step(|step| step.with_id("s1").with_act(|act| act.with_id("act2")));

    main.print();
    let main_pid = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut main, &main_pid);

    Manager::new(&scher).deploy(&w2).unwrap();
    emitter.on_message(move |e| {
        if e.is_key("act2") && e.is_state("created") {
            let options = Vars::new();
            e.do_action(&e.proc_id, &e.id, consts::EVT_SKIP, &options)
                .unwrap();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();

    // sub workflow's skip does not affect the parent act state
    assert_eq!(
        proc.task_by_nid("act1").get(0).unwrap().state(),
        TaskState::Success
    );
}

#[tokio::test]
async fn sch_act_use_act_abort() {
    let mut main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1").with_act(|act| {
            act.with_id("act1")
                .with_use("w2")
                .with_input("pid", json!("sub1"))
        })
    });

    let w2 = Workflow::new()
        .with_id("w2")
        .with_step(|step| step.with_id("s1").with_act(|act| act.with_id("act2")));

    main.print();
    let main_pid = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut main, &main_pid);

    Manager::new(&scher).deploy(&w2).unwrap();
    emitter.on_message(move |e| {
        if e.is_key("act2") && e.is_state("created") {
            let options = Vars::new();
            e.do_action(&e.proc_id, &e.id, consts::EVT_ABORT, &options)
                .unwrap();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").get(0).unwrap().state(),
        TaskState::Abort
    );
}

#[tokio::test]
async fn sch_act_use_act_error() {
    let mut main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1").with_act(|act| {
            act.with_id("act1")
                .with_use("w2")
                .with_input("pid", json!("sub1"))
        })
    });

    let w2 = Workflow::new()
        .with_id("w2")
        .with_step(|step| step.with_id("s1").with_act(|act| act.with_id("act2")));

    main.print();
    let main_pid = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut main, &main_pid);

    Manager::new(&scher).deploy(&w2).unwrap();
    emitter.on_message(move |e| {
        if e.is_key("act2") && e.is_state("created") {
            let mut options = Vars::new();
            options.insert("err_code".to_string(), "sub workflow error".into());
            e.do_action(&e.proc_id, &e.id, consts::EVT_ERR, &options)
                .unwrap();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert!(proc.task_by_nid("act1").get(0).unwrap().state().is_error(),);
}

fn create_proc(workflow: &mut Workflow, pid: &str) -> (Arc<Proc>, Arc<Scheduler>, Arc<Emitter>) {
    let engine = Engine::new();
    let scher = engine.scher();

    let proc = Arc::new(Proc::new(&pid));
    proc.load(workflow).unwrap();

    // engine.manager().deploy(&workflow).unwrap();

    // let mut options = Vars::new();
    // options.insert("pid".to_string(), json!(pid));
    // engine.executor().start(&workflow.id, &options).unwrap();

    let emitter = scher.emitter().clone();
    emitter.on_complete(move |p| {
        println!("on_complete: {p:?}");
        if p.mid == "main" && p.state.is_completed() {
            p.close();
        }
    });

    emitter.on_error(move |p| {
        println!("error in '{}', error={}", p.inner().pid, p.inner().state);
        p.close();
    });
    (proc, scher, emitter)
}
