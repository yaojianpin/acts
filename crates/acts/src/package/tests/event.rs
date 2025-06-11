use crate::{Act, Workflow};
use crate::{Engine, Vars, query::Query};
use serde_json::json;

#[tokio::test]
async fn pack_event_deploy() {
    let engine = Engine::new().start();
    let workflow = Workflow::new()
        .with_id("my-event-model")
        .with_on(|act| {
            act.with_id("event1")
                .with_uses("acts.event.manual")
                .with_params_vars(|vars| vars.with("test", 10))
        })
        .with_on(|act| {
            act.with_id("event2")
                .with_uses("acts.event.manual")
                .with_params_vars(|vars| vars.with("test", 20))
        })
        .with_step(|step| step.with_id("step1"));

    workflow.print();

    engine.executor().model().deploy(&workflow).unwrap();
    let ret = engine
        .executor()
        .evt()
        .list(&Query::new().limit(100))
        .unwrap();

    assert_eq!(ret.count, 2);
}

#[tokio::test]
async fn pack_event_get() {
    let engine = Engine::new().start();
    let workflow = Workflow::new()
        .with_id("my-event-model")
        .with_on(|act| {
            act.with_id("event1")
                .with_uses("acts.event.manual")
                .with_params_vars(|vars| vars.with("test", 10))
        })
        .with_on(|act| {
            act.with_id("event2")
                .with_uses("acts.event.hook")
                .with_params_vars(|vars| vars.with("test", 20))
        })
        .with_step(|step| step.with_id("step1"));

    workflow.print();

    engine.executor().model().deploy(&workflow).unwrap();
    let ret = engine
        .executor()
        .evt()
        .get("my-event-model:event1")
        .unwrap();

    assert_eq!(ret.uses, "acts.event.manual");

    let ret = engine
        .executor()
        .evt()
        .get("my-event-model:event2")
        .unwrap();
    assert_eq!(ret.uses, "acts.event.hook");
}

#[tokio::test]
async fn pack_event_manual_start() {
    let engine = Engine::new().start();
    let workflow = Workflow::new()
        .with_id("my-event-model")
        .with_on(|act| {
            act.with_id("event1")
                .with_uses("acts.event.manual")
                .with_params_vars(|vars| vars.with("test", 10))
        })
        .with_step(|step| step.with_id("step1"));

    workflow.print();

    engine.executor().model().deploy(&workflow).unwrap();
    let ret = engine
        .executor()
        .evt()
        .start("my-event-model:event1", &Vars::new().into())
        .await
        .unwrap();

    assert!(ret.unwrap().get::<String>("pid").is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn pack_event_hook_start() {
    let engine = Engine::new().start();
    let workflow = Workflow::new()
        .with_input("ret", 0.into())
        .with_output("ret", json!(null))
        .with_id("my-event-model")
        .with_on(|act| {
            act.with_id("event1")
                .with_uses("acts.event.hook")
                .with_params_vars(|vars| vars.with("var1", 10))
        })
        .with_step(|step| {
            step.with_id("step1")
                .with_act(Act::set(Vars::new().with("ret", 100)))
        });

    workflow.print();
    engine.executor().model().deploy(&workflow).unwrap();
    let ret = engine
        .executor()
        .evt()
        .start("my-event-model:event1", &Vars::new().into())
        .await
        .unwrap();

    println!("event ret: {:?}", ret);
    assert_eq!(ret.unwrap().get::<i32>("ret").unwrap(), 100);
}

#[tokio::test]
async fn pack_event_chat_start() {
    let engine = Engine::new().start();
    let workflow = Workflow::new()
        .with_input("ret", 0.into())
        .with_output("ret", json!(null))
        .with_id("my-event-model")
        .with_on(|act| act.with_id("event1").with_uses("acts.event.chat"))
        .with_step(|step| step.with_id("step1"));

    workflow.print();
    engine.executor().model().deploy(&workflow).unwrap();
    let ret = engine
        .executor()
        .evt()
        .start("my-event-model:event1", &"hello".into())
        .await
        .unwrap();

    println!("event ret: {:?}", ret);
    assert!(ret.unwrap().get::<String>("pid").is_some());
}

#[tokio::test]
async fn pack_event_multiple() {
    let engine = Engine::new().start();
    let workflow = Workflow::new()
        .with_id("my-event-model")
        .with_on(|act| {
            act.with_id("event1")
                .with_uses("acts.event.manual")
                .with_params_vars(|vars| vars.with("test", 10))
        })
        .with_on(|act| {
            act.with_id("event2")
                .with_uses("acts.event.manual")
                .with_params_vars(|vars| vars.with("test", 20))
        })
        .with_step(|step| step.with_id("step1"));

    workflow.print();

    engine.executor().model().deploy(&workflow).unwrap();
    let ret = engine
        .executor()
        .evt()
        .start("my-event-model:event1", &Vars::new().into())
        .await
        .unwrap();

    assert!(ret.unwrap().get::<String>("pid").is_some());

    let ret = engine
        .executor()
        .evt()
        .start("my-event-model:event2", &Vars::new().into())
        .await
        .unwrap();

    assert!(ret.unwrap().get::<String>("pid").is_some());
}

#[tokio::test]
async fn pack_event_dup_id() {
    let engine = Engine::new().start();
    let workflow = r#"
    id: "my-event-model"
    on:
      - id: event1
        uses: acts.event.manual
        params:
          test: 10
      - id: event1
        uses: acts.event.manual
        params:
          test: 20
    steps:
        - id: step1
    "#;
    let workflow = Workflow::from_yml(workflow).unwrap();
    let ret = engine.executor().model().deploy(&workflow);
    assert!(ret.is_err());
}

#[tokio::test]
async fn pack_event_empty_id() {
    let engine = Engine::new().start();
    let workflow = r#"
    id: "my-event-model"
    on:
      - uses: acts.event.manual
        params:
          test: 10
    steps:
        - id: step1
    "#;
    let workflow = Workflow::from_yml(workflow).unwrap();
    let ret = engine.executor().model().deploy(&workflow);
    assert!(ret.is_err());
}
