use crate::cache::Cache;
use crate::event::EventAction;
use crate::scheduler::Runtime;
use crate::utils::test::auto_complete;
use crate::{Act, Action, Config, Engine, MessageState, TaskState, Vars, Workflow, utils};
use serde_json::json;

use serial_test::serial;
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_scher_next() {
    let config = Config::default();
    let runtime = Runtime::new(&config).unwrap();
    let cache = Cache::new(&config).unwrap();
    let store = cache.store();
    let workflow = Workflow::new().with_id(&utils::longid());

    let s = runtime.clone();
    store.deploy(&workflow).unwrap();
    tokio::spawn(async move {
        let mut options = Vars::new();
        options.insert("pid".to_string(), json!(utils::longid()));
        s.start(&workflow, options).unwrap();
    });

    let ret = runtime.queue().next().await;
    assert!(ret.is_ok());
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_scher_task() {
    let config = Config::default();
    let runtime = Runtime::new(&config).unwrap();
    let workflow = Workflow::new();
    let pid = utils::longid();
    let proc = runtime.create_proc(&pid, &workflow);
    proc.set_state(TaskState::Pending);
    runtime.launch(&proc).unwrap();
    let ret = runtime.queue().next().await;
    assert!(ret.is_ok());
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_scher_start_default() {
    let config = Config::default();
    let runtime = Runtime::new(&config).unwrap();
    let workflow = Workflow::new();
    let result = runtime.start(&workflow, Vars::new());
    assert!(result.is_ok());
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_scher_start_with_vars() {
    let config = Config::default();
    let runtime = Runtime::new(&config).unwrap();
    let workflow = Workflow::new();
    let mut vars = Vars::new();
    vars.insert("a".to_string(), json!(100));
    vars.insert("b".to_string(), json!("string"));

    let proc = runtime.start(&workflow, vars).unwrap();
    let _ = runtime.queue().next().await;

    assert_eq!(proc.inputs().get::<i64>("a").unwrap(), 100);
    assert_eq!(proc.inputs().get::<String>("b").unwrap(), "string");
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_scher_do_action() {
    let engine = Engine::new().start().unwrap();
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    let workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(Act::irq(|act| {
            act.with_key("act1").with_expose("uid", json!(null))
        }))
    });
    auto_complete(&engine, &rx);
    engine.channel().on_message(move |e| {
        if e.is_key("act1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            let action = Action::new(&e.pid, &e.tid, EventAction::Next, options);
            rt.do_action(&action).unwrap();
        }
    });
    let proc = engine.runtime().create_proc(&utils::longid(), &workflow);
    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;

    assert!(proc.state().is_success());
}
