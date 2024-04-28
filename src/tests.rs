use crate::{utils, Act, Builder, Engine, Vars, Workflow};
use serde_json::json;

#[tokio::test]
async fn engine_start() {
    let engine = Engine::new();
    assert!(engine.is_running());
}

#[tokio::test]
async fn engine_event_on_message() {
    let engine = Engine::new();
    let sig = engine.signal("".to_string());
    let s = sig.clone();
    let mid = utils::longid();
    let workflow = Workflow::new()
        .with_id(&mid)
        .with_step(|step| step.with_act(Act::req(|act| act.with_id("test"))));

    engine.emitter().on_message(move |e| {
        if e.is_source("act") {
            s.update(|data| *data = e.key.clone());
            s.close();
        }
    });

    let executor = engine.executor();
    engine.manager().deploy(&workflow).unwrap();

    let mut options = Vars::new();
    options.insert("pid".to_string(), json!(utils::longid()));
    executor.start(&workflow.id, &options).unwrap();
    let ret = sig.recv().await;
    assert_eq!(ret, "test");
}

#[tokio::test]
async fn engine_event_on_start() {
    let engine = Engine::new();

    let sig = engine.signal("".to_string());
    let s = sig.clone();
    let mid = utils::longid();
    let workflow = Workflow::new()
        .with_id(&mid)
        .with_step(|step| step.with_act(Act::req(|act| act.with_id("test"))));

    engine.emitter().on_start(move |e| {
        s.send(e.mid.clone());
    });

    let executor = engine.executor();
    engine.manager().deploy(&workflow).unwrap();

    let mut options = Vars::new();
    options.insert("pid".to_string(), json!(utils::longid()));
    executor.start(&workflow.id, &options).unwrap();
    let ret = sig.recv().await;
    assert_eq!(ret, mid);
}

#[tokio::test]
async fn engine_event_on_complete() {
    let engine = Engine::new();
    let sig = engine.signal(false);
    let s1 = sig.clone();
    let mid = utils::longid();
    let workflow = Workflow::new()
        .with_id(&mid)
        .with_step(|step| step.with_id("step1"));

    engine.emitter().on_complete(move |e| {
        s1.send(e.mid == mid);
    });

    let executor = engine.executor();
    engine.manager().deploy(&workflow).unwrap();

    let mut options = Vars::new();
    options.insert("pid".to_string(), json!(utils::longid()));
    executor.start(&workflow.id, &options).unwrap();
    let ret = sig.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn engine_event_on_error() {
    let engine = Engine::new();
    let mid = utils::longid();
    let workflow = Workflow::new().with_id(&mid).with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|a| a.with_id("act1")))
    });

    let sig = engine.signal(false);
    let s1 = sig.clone();
    engine.emitter().on_error(move |e| {
        s1.send(e.mid == mid);
    });

    engine.emitter().on_message(move |e| {
        let mut options = Vars::new();
        options.insert("uid".to_string(), json!("u1"));
        options.insert("error".to_string(), json!({ "ecode": "err1" }));

        if e.is_key("act1") && e.is_state("created") {
            e.do_action(&e.proc_id, &e.id, "error", &options).unwrap();
        }
    });

    let executor = engine.executor();
    engine.manager().deploy(&workflow).unwrap();

    let mut options = Vars::new();
    options.insert("pid".to_string(), json!(utils::longid()));
    executor.start(&workflow.id, &options).unwrap();
    let ret = sig.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn engine_model_create() {
    let workflow = Workflow::new()
        .with_name("w1")
        .with_input("v", 0.into())
        .with_step(|step| {
            step.with_id("step1")
                .with_name("step1")
                .with_run(r#"print("step1")"#)
                .with_branch(|branch| {
                    branch
                        .with_if(r#"${ $("v") > 100 }"#)
                        .with_step(|step| step.with_name("step3").with_run(r#"print("step3")"#))
                })
                .with_branch(|branch| {
                    branch
                        .with_if(r#"${ $("v") <= 100 }"#)
                        .with_step(|step| step.with_name("step4").with_run(r#"print("step4")"#))
                })
        })
        .with_step(|step| step.with_name("step2").with_run(r#"print("step2")"#));

    assert_eq!(workflow.name, "w1");
    let step = workflow.step("step1").unwrap();
    assert_eq!(step.name, "step1");
    assert_eq!(step.branches.len(), 2);
}

#[tokio::test]
async fn engine_build_cache_size() {
    let engine = Builder::new().cache_size(100).build();
    assert_eq!(engine.config().cache_cap, 100)
}

#[tokio::test]
async fn engine_build_data_dir() {
    let engine = Builder::new().data_dir("test").build();
    assert_eq!(engine.config().data_dir, "test")
}

#[tokio::test]
async fn engine_build_db_name() {
    let engine = Builder::new().db_name("test.db").build();
    assert_eq!(engine.config().db_name, "test.db")
}

#[tokio::test]
async fn engine_build_log_dir() {
    let engine = Builder::new().log_dir("test").build();
    assert_eq!(engine.config().log_dir, "test")
}

#[tokio::test]
async fn engine_build_log_level() {
    let engine = Builder::new().log_level("DEBUG").build();
    assert_eq!(engine.config().log_level, "DEBUG")
}

#[tokio::test]
async fn engine_build_tick_interval_secs() {
    let engine = Builder::new().tick_interval_secs(10).build();
    assert_eq!(engine.config().tick_interval_secs, 10)
}
