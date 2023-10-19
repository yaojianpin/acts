use crate::{utils, Engine, Vars, Workflow};
use serde_json::json;

#[tokio::test]
async fn engine_start() {
    let engine = Engine::new();

    let e = engine.clone();
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        e.close();
    });
    engine.start();
    assert!(true);
}

#[tokio::test]
async fn engine_start_async() {
    let engine = Engine::new();
    engine.start();
    let e = engine.clone();
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        e.close();
    });

    engine.eloop().await;

    assert!(true);
}

#[tokio::test]
async fn engine_event_on_message() {
    let engine = Engine::new();
    engine.start();

    let mid = utils::longid();
    let workflow = Workflow::new().with_id(&mid).with_job(|job| {
        job.with_id("job1")
            .with_step(|step| step.with_act(|act| act.with_id("test")))
    });

    engine.emitter().on_message(move |e| {
        if e.is_type("act") {
            assert_eq!(e.id, "test");
        }

        e.close();
    });

    let executor = engine.executor();
    engine.manager().deploy(&workflow).unwrap();

    let mut options = Vars::new();
    options.insert("pid".to_string(), json!(utils::longid()));
    executor.start(&workflow.id, &options).unwrap();
    engine.eloop().await;
}

#[tokio::test]
async fn engine_event_on_start() {
    let engine = Engine::new();
    engine.start();

    let mid = utils::longid();
    let workflow = Workflow::new().with_id(&mid).with_job(|job| {
        job.with_id("job1")
            .with_step(|step| step.with_act(|act| act.with_id("test")))
    });

    engine.emitter().on_start(move |e| {
        assert_eq!(e.mid, mid);
        e.close();
    });

    let executor = engine.executor();
    engine.manager().deploy(&workflow).unwrap();

    let mut options = Vars::new();
    options.insert("pid".to_string(), json!(utils::longid()));
    executor.start(&workflow.id, &options).unwrap();
    engine.eloop().await;
}

#[tokio::test]
async fn engine_event_on_complete() {
    let engine = Engine::new();
    engine.start();

    let mid = utils::longid();
    let workflow = Workflow::new()
        .with_id(&mid)
        .with_job(|job| job.with_id("job1").with_step(|step| step.with_id("step1")));

    engine.emitter().on_complete(move |e| {
        assert_eq!(e.mid, mid);
        e.close();
    });

    let executor = engine.executor();
    engine.manager().deploy(&workflow).unwrap();

    let mut options = Vars::new();
    options.insert("pid".to_string(), json!(utils::longid()));
    executor.start(&workflow.id, &options).unwrap();
    engine.eloop().await;
}

#[tokio::test]
async fn engine_event_on_error() {
    let engine = Engine::new();
    engine.start();

    let mid = utils::longid();
    let workflow = Workflow::new().with_id(&mid).with_job(|job| {
        job.with_id("job1")
            .with_step(|step| step.with_id("step1").with_act(|a| a.with_id("act1")))
    });

    engine.emitter().on_error(move |e| {
        assert_eq!(e.mid, mid);
        e.close();
    });

    engine.emitter().on_message(move |e| {
        let mut options = Vars::new();
        options.insert("uid".to_string(), json!("u1"));
        options.insert("err_code".to_string(), json!("err1"));

        if e.is_key("act1") && e.is_state("created") {
            e.do_action(&e.proc_id, &e.id, "error", &options).unwrap();
        }
    });

    let executor = engine.executor();
    engine.manager().deploy(&workflow).unwrap();

    let mut options = Vars::new();
    options.insert("pid".to_string(), json!(utils::longid()));
    executor.start(&workflow.id, &options).unwrap();
    engine.eloop().await;
}

#[tokio::test]
async fn engine_builder() {
    let workflow = Workflow::new().with_name("w1").with_job(|job| {
        job.with_id("job1")
            .with_name("job 1")
            .with_input("v", 0.into())
            .with_step(|step| {
                step.with_id("step1")
                    .with_name("step1")
                    .with_run(r#"print("step1")"#)
                    .with_branch(|branch| {
                        branch
                            .with_if(r#"${ env.get("v") > 100 }"#)
                            .with_step(|step| step.with_name("step3").with_run(r#"print("step3")"#))
                    })
                    .with_branch(|branch| {
                        branch
                            .with_if(r#"${ env.get("v") <= 100 }"#)
                            .with_step(|step| step.with_name("step4").with_run(r#"print("step4")"#))
                    })
            })
            .with_step(|step| step.with_name("step2").with_run(r#"print("step2")"#))
    });

    assert_eq!(workflow.name, "w1");

    let job = workflow.job("job1").unwrap();
    assert_eq!(job.name, "job 1");
    assert_eq!(job.steps.len(), 2);

    let step = job.step("step1").unwrap();
    assert_eq!(step.name, "step1");
    assert_eq!(step.branches.len(), 2);
}
