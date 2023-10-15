use crate::{
    sch::TaskState,
    store::{data, StoreAdapter},
    utils, ActPlugin, Engine, Vars, Workflow,
};
use rhai::plugin::*;
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
async fn engine_register_plugin() {
    let engine = Engine::new();
    let extender = engine.extender();

    let plugin_count = extender.plugins.lock().unwrap().len();
    extender.register_plugin(&TestPlugin::default());

    assert_eq!(extender.plugins.lock().unwrap().len(), plugin_count + 1);
}

#[tokio::test]
async fn engine_register_module() {
    let engine = Engine::new();
    let extender = engine.extender();
    let mut module = Module::new();
    combine_with_exported_module!(&mut module, "role", test_module);
    extender.register_module("test", &module);

    assert!(extender.modules().contains_key("test"));
}

#[tokio::test]
async fn engine_on_message() {
    let engine = Engine::new();
    engine.start();

    let mid = utils::longid();
    let workflow = Workflow::new().with_id(&mid).with_job(|job| {
        job.with_id("job1")
            .with_step(|step| step.with_act(|act| act.with_id("test")))
    });

    let e = engine.clone();
    engine.emitter().on_message(move |msg| {
        if msg.inner().is_type("act") {
            assert_eq!(msg.inner().id, "test");
        }

        e.close();
    });

    let executor = engine.executor();
    engine
        .manager()
        .deploy(&workflow)
        .expect("fail to deploy workflow");

    let mut options = Vars::new();
    options.insert("pid".to_string(), json!(utils::longid()));
    executor
        .start(&workflow.id, &options)
        .expect("fail to start workflow");
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

#[tokio::test]
async fn engine_executor_start_no_pid() {
    let engine = Engine::new();
    let executor = engine.executor();
    engine.start();

    let mid = utils::longid();
    let workflow = Workflow::new()
        .with_id(&mid)
        .with_job(|job| job.with_step(|step| step.with_act(|act| act.with_id("test"))));
    engine.manager().deploy(&workflow).unwrap();
    let options = Vars::new();
    let result = executor.start(&workflow.id, &options);
    assert_eq!(result.is_ok(), true);
}

#[tokio::test]
async fn engine_executor_start_empty_pid() {
    let engine = Engine::new();
    let executor = engine.executor();
    engine.start();

    let mid = utils::longid();
    let workflow = Workflow::new()
        .with_id(&mid)
        .with_job(|job| job.with_step(|step| step.with_act(|act| act.with_id("test"))));

    engine.manager().deploy(&workflow).unwrap();
    let mut options = Vars::new();
    options.insert("pid".to_string(), "".into());
    let result = executor.start(&workflow.id, &options);
    assert_eq!(result.is_ok(), true);
}

#[tokio::test]
async fn engine_executor_start_dup_pid_error() {
    let engine = Engine::new();
    let executor = engine.executor();
    engine.start();

    let pid = utils::longid();
    let mid = utils::longid();
    let model = Workflow::new()
        .with_id(&mid)
        .with_job(|job| job.with_step(|step| step.with_act(|act| act.with_id("test"))));

    let store = engine.scher().cache().store();
    let proc = data::Proc {
        id: pid.clone(),
        name: model.name.clone(),
        mid: model.id.clone(),
        state: TaskState::None.to_string(),
        start_time: 0,
        end_time: 0,
        vars: "".to_string(),
        timestamp: 0,
        model: model.to_json().unwrap(),
    };
    store.procs().create(&proc).expect("create proc");
    engine
        .manager()
        .deploy(&model)
        .expect("fail to deploy workflow");
    let mut options = Vars::new();
    options.insert("pid".to_string(), json!(pid.to_string()));
    let result = executor.start(&model.id, &options);
    assert_eq!(result.is_err(), true);
}

#[tokio::test]
async fn engine_manager_models() {
    let engine = Engine::new();
    engine.start();
    let manager = engine.manager();
    let mid = utils::longid();
    let workflow = Workflow::new()
        .with_id(&mid)
        .with_job(|job| job.with_step(|step| step.with_act(|act| act.with_id("test"))));

    engine.manager().deploy(&workflow).expect("deploy model");
    let models = manager.models(100).expect("get models");
    assert!(models.len() > 0);
}

#[tokio::test]
async fn engine_manager_model() {
    let engine = Engine::new();
    engine.start();
    let manager = engine.manager();
    let mid: String = utils::longid();
    let mut workflow = Workflow::new()
        .with_id(&mid)
        .with_job(|job| job.with_step(|step| step.with_act(|act| act.with_id("test"))));
    workflow.id = utils::longid();
    manager.deploy(&workflow).expect("deploy model");

    let model = manager.model(&workflow.id, "text");
    assert_eq!(model.is_ok(), true);
}

#[tokio::test]
async fn engine_manager_procs() {
    let engine = Engine::new();
    engine.start();
    let manager = engine.manager();

    let mid: String = utils::longid();
    let model = Workflow::new()
        .with_id(&mid)
        .with_job(|job| job.with_step(|step| step.with_act(|act| act.with_id("test"))));

    let store = engine.scher().cache().store();
    let proc_id = utils::longid();
    let proc = data::Proc {
        id: proc_id.clone(),
        model: model.to_json().unwrap(),
        name: model.name,
        mid: model.id,
        state: TaskState::None.to_string(),
        start_time: 0,
        end_time: 0,
        vars: "".to_string(),
        timestamp: 0,
    };
    store.procs().create(&proc).expect("create proc");

    let procs = manager.procs(100).expect("get procs");
    assert!(procs.len() > 0);
}

#[tokio::test]
async fn engine_manager_proc() {
    let engine = Engine::new();
    engine.start();
    let manager = engine.manager();
    let pid = utils::longid();
    let mid: String = utils::longid();
    let model = Workflow::new()
        .with_id(&mid)
        .with_job(|job| job.with_step(|step| step.with_act(|act| act.with_id("test"))));

    let store = engine.scher().cache().store();
    let proc = data::Proc {
        id: pid.clone(),
        model: model.to_json().unwrap(),
        name: model.name,
        mid: model.id,
        state: TaskState::None.to_string(),
        start_time: 0,
        end_time: 0,
        vars: "".to_string(),
        timestamp: 0,
    };
    store.procs().create(&proc).expect("create proc");

    let proc = manager.proc(&pid, "json");
    assert_eq!(proc.is_ok(), true);
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
