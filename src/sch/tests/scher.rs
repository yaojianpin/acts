use crate::{
    sch::{Proc, Scheduler},
    utils::{self, consts},
    Act, Action, Engine, TaskState, Vars, Workflow,
};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn sch_scher_next() {
    let engine = Engine::new();
    let store = engine.scher().cache().store();
    let scher = engine.scher();
    let workflow = Workflow::new().with_id(&utils::longid());

    let s = scher.clone();
    store.deploy(&workflow).unwrap();
    tokio::spawn(async move {
        let mut options = Vars::new();
        options.insert("pid".to_string(), json!(utils::longid()));
        s.start(&workflow, &options).unwrap();
    });

    let ret = scher.next().await;
    assert_eq!(ret, true);
}

#[tokio::test]
async fn sch_scher_task() {
    let engine = Engine::new();
    let scher = engine.scher();
    let workflow = Workflow::new();
    let pid = utils::longid();
    let s = scher.clone();
    tokio::spawn(async move {
        let proc = Proc::new(&pid);
        proc.load(&workflow).unwrap();
        proc.set_state(TaskState::Pending);
        s.launch(&Arc::new(proc))
    });

    let ret = scher.next().await;
    assert_eq!(ret, true);
}

#[tokio::test]
async fn sch_scher_start_default() {
    let scher = Scheduler::new();
    let workflow = Workflow::new();
    let s = scher.clone();
    let result = s.start(&workflow, &Vars::new());
    assert_eq!(result.is_ok(), true);
}

#[tokio::test]
async fn sch_scher_start_with_vars() {
    let engine = Engine::new();
    let scher = engine.scher();
    let workflow = Workflow::new();
    let s = scher.clone();
    let mut vars = Vars::new();
    vars.insert("a".to_string(), json!(100));
    vars.insert("b".to_string(), json!("string"));

    let proc = s.start(&workflow, &vars).unwrap();
    scher.next().await;

    assert_eq!(proc.inputs().get::<i64>("a").unwrap(), 100);
    assert_eq!(proc.inputs().get::<String>("b").unwrap(), "string");
}

#[tokio::test]
async fn sch_scher_do_action() {
    let engine = Engine::new();
    let scher = engine.scher();
    let sig = scher.signal(());
    let rx = sig.clone();
    let workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(Act::req(|act| {
            act.with_id("act1").with_input("uid", json!("u1"))
        }))
    });
    let s = scher.clone();
    scher.emitter().on_complete(move |_| rx.close());
    scher.emitter().on_message(move |e| {
        if e.is_key("act1") && e.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            let action = Action::new(&e.proc_id, &e.id, consts::EVT_NEXT, &options);
            s.do_action(&action).unwrap();
        }
    });
    let proc = Arc::new(Proc::new(&utils::longid()));
    proc.load(&workflow).unwrap();
    scher.launch(&proc);
    sig.recv().await;

    assert_eq!(proc.state().is_success(), true);
}
