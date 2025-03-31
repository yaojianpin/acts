use crate::event::EventAction;
use crate::{utils, Act, Action, Engine, MessageState, TaskState, Vars, Workflow};
use serde_json::json;

#[tokio::test]
async fn sch_scher_next() {
    let engine = Engine::new();

    let rt = engine.runtime();
    let store = rt.cache().store();
    let workflow = Workflow::new().with_id(&utils::longid());

    let s = rt.clone();
    store.deploy(&workflow).unwrap();
    tokio::spawn(async move {
        let mut options = Vars::new();
        options.insert("pid".to_string(), json!(utils::longid()));
        s.start(&workflow, &options).unwrap();
    });

    let ret = rt.scher().next().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_scher_task() {
    let engine = Engine::new();
    let rt = engine.runtime();
    let workflow = Workflow::new();
    let pid = utils::longid();
    let s = rt.clone();
    tokio::spawn(async move {
        let proc = s.create_proc(&pid, &workflow);
        proc.set_state(TaskState::Pending);
        s.launch(&proc)
    });

    let ret = rt.scher().next().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_scher_start_default() {
    let engine = Engine::new();
    let rt = engine.runtime();
    let workflow = Workflow::new();
    let result = rt.start(&workflow, &Vars::new());
    assert!(result.is_ok());
}

#[tokio::test]
async fn sch_scher_start_with_vars() {
    let engine = Engine::new();
    let rt = engine.runtime();
    let workflow = Workflow::new();
    let mut vars = Vars::new();
    vars.insert("a".to_string(), json!(100));
    vars.insert("b".to_string(), json!("string"));

    let proc = rt.start(&workflow, &vars).unwrap();
    rt.scher().next().await;

    assert_eq!(proc.inputs().get::<i64>("a").unwrap(), 100);
    assert_eq!(proc.inputs().get::<String>("b").unwrap(), "string");
}

#[tokio::test]
async fn sch_scher_do_action() {
    let engine = Engine::new();
    let rt = engine.runtime();
    let sig = engine.signal(());
    let rx = sig.clone();
    let workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(Act::irq(|act| {
            act.with_key("act1").with_ret("uid", json!("u1"))
        }))
    });
    let s = rt.clone();
    engine.channel().on_complete(move |_| rx.close());
    engine.channel().on_message(move |e| {
        if e.is_key("act1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            let action = Action::new(&e.pid, &e.tid, EventAction::Next, &options);
            s.do_action(&action).unwrap();
        }
    });
    let proc = rt.create_proc(&utils::longid(), &workflow);
    rt.launch(&proc);
    sig.recv().await;

    assert!(proc.state().is_success());
}
