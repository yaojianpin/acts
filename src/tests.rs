use crate::{sch::Event, ActPlugin, Engine, Message, State, Workflow};
use rhai::plugin::*;
use std::sync::Arc;

#[tokio::test]
async fn engine_start() {
    let engine = Engine::new();

    let e = engine.clone();
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        e.close();
    });
    engine.start().await;
    assert!(true);
}

#[tokio::test]
async fn engine_start_async() {
    let engine = Engine::new();

    let e = engine.clone();
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        e.close();
    });

    tokio::spawn(async move {
        engine.start().await;
    });

    assert!(true);
}

#[tokio::test]
async fn engine_register_plugin() {
    let engine = Engine::new();
    let mgr = engine.mgr();

    let plugin_count = mgr.plugins.lock().unwrap().len();
    mgr.register_plugin(&TestPlugin::default());

    assert_eq!(mgr.plugins.lock().unwrap().len(), plugin_count + 1);
}

#[tokio::test]
async fn engine_register_action() {
    let engine = Engine::new();
    let mgr = engine.mgr();
    let add = |a: i64, b: i64| Ok(a + b);
    let hash = mgr.register_action("add", add);

    assert!(mgr.action().contains_fn(hash));
}

#[tokio::test]
async fn engine_register_module() {
    let engine = Engine::new();
    let mgr = engine.mgr();
    let mut module = Module::new();
    combine_with_exported_module!(&mut module, "role", test_module);
    mgr.register_module("test", &module);

    assert!(mgr.modules().contains_key("test"));
}

#[tokio::test]
async fn engine_on_message() {
    let engine = Engine::new();
    let workflow = Workflow::new().with_job(|job| {
        job.with_step(|step| {
            step.with_subject(|sub| sub.with_matcher("any").with_users(r#"["a"]"#))
        })
    });

    println!("{}", workflow.to_string().unwrap());
    let e = engine.clone();
    engine.emitter().on_message(move |msg: &Message| {
        assert_eq!(msg.uid, Some("a".to_string()));
        e.close();
    });

    let executor = engine.executor();
    executor.start(&workflow);
    engine.start().await;
}

#[tokio::test]
async fn engine_builder() {
    let workflow = Workflow::new().with_name("w1").with_job(|job| {
        job.with_id("job1")
            .with_name("job 1")
            .with_env("v", 0.into())
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

#[derive(Debug, Default, Clone)]
struct TestPlugin;

impl ActPlugin for TestPlugin {
    fn on_init(&self, _engine: &Engine) {
        println!("TestPlugin");
    }
}

#[export_module]
mod test_module {

    #[export_fn]
    pub fn test(_name: &str) {}
}
