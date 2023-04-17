use crate::{
    event::Message,
    sch::TaskState,
    store::{self, StoreAdapter},
    utils, ActPlugin, Engine, Workflow,
};
use rhai::plugin::*;

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
    engine.start().await;
    let e = engine.clone();
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        e.close();
    });

    engine.r#loop().await;

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
async fn engine_register_action() {
    let engine = Engine::new();
    let extender = engine.extender();
    let add = |a: i64, b: i64| Ok(a + b);
    let hash = extender.register_action("add", add);

    assert!(extender.action().contains_fn(hash));
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
    engine.start().await;
    let workflow = Workflow::new().with_id("m1").with_job(|job| {
        job.with_id("job1").with_step(|step| {
            step.with_subject(|sub| sub.with_matcher("any").with_users(r#"["a"]"#))
        })
    });

    // workflow.print_tree().unwrap();
    let e = engine.clone();
    engine.emitter().on_message(move |msg: &Message| {
        assert_eq!(msg.uid, Some("a".to_string()));
        e.close();
    });

    let executor = engine.executor();
    executor.deploy(&workflow).expect("fail to deploy workflow");
    executor
        .start(
            &workflow.id,
            crate::ActionOptions {
                biz_id: Some(utils::longid()),
                ..Default::default()
            },
        )
        .expect("fail to start workflow");
    engine.r#loop().await;
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

#[tokio::test]
async fn engine_executor_start_no_biz_id() {
    let engine = Engine::new();
    let executor = engine.executor();
    engine.start().await;
    let workflow = Workflow::new().with_id("m1").with_job(|job| {
        job.with_step(|step| {
            step.with_subject(|sub| sub.with_matcher("any").with_users(r#"["a"]"#))
        })
    });

    let result = executor.start(
        &workflow.id,
        crate::ActionOptions {
            biz_id: None,
            ..Default::default()
        },
    );
    assert_eq!(result.is_ok(), true);
}

#[tokio::test]
async fn engine_executor_start_empty_biz_id() {
    let engine = Engine::new();
    let executor = engine.executor();
    engine.start().await;
    let workflow = Workflow::new().with_id("m1").with_job(|job| {
        job.with_step(|step| {
            step.with_subject(|sub| sub.with_matcher("any").with_users(r#"["a"]"#))
        })
    });

    let result = executor.start(
        &workflow.id,
        crate::ActionOptions {
            biz_id: Some("".to_string()),
            ..Default::default()
        },
    );
    assert_eq!(result.is_ok(), true);
}

#[tokio::test]
async fn engine_executor_start_dup_biz_id_error() {
    let engine = Engine::new();
    let executor = engine.executor();
    engine.start().await;

    let biz_id = utils::longid();
    let model = Workflow::new().with_id("m1").with_job(|job| {
        job.with_step(|step| {
            step.with_subject(|sub| sub.with_matcher("any").with_users(r#"["a"]"#))
        })
    });

    let store = engine.store();
    let proc = store::Proc {
        id: biz_id.clone(),
        pid: biz_id.clone(),
        model: model.to_string().unwrap(),
        state: TaskState::None.to_string(),
        start_time: 0,
        end_time: 0,
        vars: "".to_string(),
    };
    store.procs().create(&proc).expect("create proc");
    executor.deploy(&model).expect("fail to deploy workflow");
    let result = executor.start(
        &model.id,
        crate::ActionOptions {
            biz_id: Some(biz_id.clone()),
            ..Default::default()
        },
    );
    assert_eq!(result.is_err(), true);
}

#[tokio::test]
async fn engine_manager_models() {
    let engine = Engine::new();
    engine.start().await;
    let manager = engine.manager();
    let executor = engine.executor();
    let workflow = Workflow::new().with_id("m1").with_job(|job| {
        job.with_step(|step| {
            step.with_subject(|sub| sub.with_matcher("any").with_users(r#"["a"]"#))
        })
    });

    executor.deploy(&workflow).expect("deploy model");
    let models = manager.models(100).expect("get models");
    assert!(models.len() > 0);
}

#[tokio::test]
async fn engine_manager_model() {
    let engine = Engine::new();
    engine.start().await;
    let manager = engine.manager();
    let executor = engine.executor();
    let mut workflow = Workflow::new().with_id("m1").with_job(|job| {
        job.with_step(|step| {
            step.with_subject(|sub| sub.with_matcher("any").with_users(r#"["a"]"#))
        })
    });
    workflow.id = utils::longid();
    executor.deploy(&workflow).expect("deploy model");

    let model = manager.model(&workflow.id);
    assert_eq!(model.is_ok(), true);
}

#[tokio::test]
async fn engine_manager_procs() {
    let engine = Engine::new();
    engine.start().await;
    let manager = engine.manager();
    let workflow = Workflow::new().with_id("m1").with_job(|job| {
        job.with_step(|step| {
            step.with_subject(|sub| sub.with_matcher("any").with_users(r#"["a"]"#))
        })
    });

    let store = engine.store();
    let pid = utils::longid();
    let proc = store::Proc {
        id: pid.clone(),
        pid: pid.clone(),
        model: workflow.to_string().unwrap(),
        state: TaskState::None.to_string(),
        start_time: 0,
        end_time: 0,
        vars: "".to_string(),
    };
    store.procs().create(&proc).expect("create proc");

    let procs = manager.procs(100).expect("get procs");
    assert!(procs.len() > 0);
}

#[tokio::test]
async fn engine_manager_proc() {
    let engine = Engine::new();
    engine.start().await;
    let manager = engine.manager();
    let biz_id = utils::longid();
    let workflow = Workflow::new().with_id("m1").with_job(|job| {
        job.with_step(|step| {
            step.with_subject(|sub| sub.with_matcher("any").with_users(r#"["a"]"#))
        })
    });

    let store = engine.store();
    let proc = store::Proc {
        id: biz_id.clone(),
        pid: biz_id.clone(),
        model: workflow.to_string().unwrap(),
        state: TaskState::None.to_string(),
        start_time: 0,
        end_time: 0,
        vars: "".to_string(),
    };
    store.procs().create(&proc).expect("create proc");

    let proc = manager.proc(&biz_id);
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
