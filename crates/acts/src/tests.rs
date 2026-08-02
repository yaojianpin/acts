use crate::event::EventAction;
use crate::{Engine, MessageState, Vars, Workflow, utils, utils::test::USES_IRQ};
use serde::Deserialize;
use serde_json::json;

use serial_test::serial;
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_start() {
    let engine = Engine::new().start();
    assert!(engine.is_ok());
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_event_on_message() {
    let engine = Engine::new().start().unwrap();
    let sig = engine.signal("".to_string());
    let s = sig.clone();
    let mid = utils::longid();
    let workflow = Workflow::new()
        .with_id(&mid)
        .with_step(|step| step.with_uses(USES_IRQ, Vars::new().with("key", "test")));

    engine.channel().on_message(move |e| {
        if e.is_type("act") {
            s.update(|data| *data = e.params().unwrap().get::<String>("key").unwrap());
            s.close();
        }
    });

    let executor = engine.executor();
    engine.executor().model().deploy(&workflow, None).unwrap();

    let mut options = Vars::new();
    options.insert("pid".to_string(), json!(utils::longid()));
    executor.proc().start(&workflow.id, options).unwrap();
    let ret = sig.recv().await;
    assert_eq!(ret, "test");
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_event_on_start() {
    let engine = Engine::new().start().unwrap();

    let sig = engine.signal("".to_string());
    let s = sig.clone();
    let mid = utils::longid();
    let workflow = Workflow::new()
        .with_id(&mid)
        .with_step(|step| step.with_uses(USES_IRQ, Vars::new().with("key", "test")));

    engine.channel().on_start(move |e| {
        s.send(e.mid.clone());
    });

    let executor = engine.executor();
    engine.executor().model().deploy(&workflow, None).unwrap();

    let mut options = Vars::new();
    options.insert("pid".to_string(), json!(utils::longid()));
    executor.proc().start(&workflow.id, options).unwrap();
    let ret = sig.recv().await;
    assert_eq!(ret, mid);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_event_on_complete() {
    let engine = Engine::new().start().unwrap();
    let sig = engine.signal(false);
    let s1 = sig.clone();
    let mid = utils::longid();
    let workflow = Workflow::new()
        .with_id(&mid)
        .with_step(|step| step.with_id("step1"));

    engine.channel().on_complete(move |e| {
        s1.send(e.mid == mid);
    });

    let executor = engine.executor();
    engine.executor().model().deploy(&workflow, None).unwrap();

    let mut options = Vars::new();
    options.insert("pid".to_string(), json!(utils::longid()));
    executor.proc().start(&workflow.id, options).unwrap();
    let ret = sig.recv().await;
    assert!(ret);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_event_on_error() {
    let engine = Engine::new().start().unwrap();
    let mid = utils::longid();
    let workflow = Workflow::new().with_id(&mid).with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });

    let sig = engine.signal(false);
    let s1 = sig.clone();
    engine.channel().on_error(move |e| {
        s1.send(e.mid == mid);
    });

    let rt = engine.runtime();
    engine.channel().on_message(move |e| {
        let mut options = Vars::new();
        options.insert("uid".to_string(), json!("u1"));
        options.set("ecode", "err1");

        if e.params().unwrap().get::<String>("key").as_deref() == Some("act1")
            && e.is_state(MessageState::Created)
        {
            rt.do_action2(&e.pid, &e.tid, EventAction::Error, options)
                .unwrap();
        }
    });

    let executor = engine.executor();
    executor.model().deploy(&workflow, None).unwrap();

    let mut options = Vars::new();
    options.insert("pid".to_string(), json!(utils::longid()));
    executor.proc().start(&workflow.id, options).unwrap();
    let ret = sig.recv().await;
    assert!(ret);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_model_create() {
    let workflow = Workflow::new()
        .with_name("w1")
        .with_var("v", 0)
        .with_step(|step| {
            step.with_id("step1")
                .with_name("step1")
                .with_branch(|branch| {
                    branch
                        .with_if(r#"${{ v > 100 }}"#)
                        .with_step(|step| step.with_name("step3"))
                })
                .with_branch(|branch| {
                    branch
                        .with_if(r#"${{ v <= 100 }}"#)
                        .with_step(|step| step.with_name("step4"))
                })
        })
        .with_step(|step| step.with_name("step2"));

    assert_eq!(workflow.name, "w1");
    let step = workflow.step("step1").unwrap();
    assert_eq!(step.name, "step1");
    assert_eq!(step.branches.len(), 2);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_build_cache_size() {
    let engine = Engine::builder().cache_size(100).build().start().unwrap();
    assert_eq!(engine.config().cache_cap(), 100)
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_build_log_dir() {
    let engine = Engine::builder()
        .log("test", "INFO")
        .build()
        .start()
        .unwrap();
    assert_eq!(engine.config().log().dir, "test")
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_build_log_level() {
    let engine = Engine::builder()
        .log("log", "DEBUG")
        .build()
        .start()
        .unwrap();
    assert_eq!(engine.config().log().level, "DEBUG")
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_build_tick_interval_secs() {
    let engine = Engine::builder()
        .tick_interval_secs(10)
        .build()
        .start()
        .unwrap();
    assert_eq!(engine.config().tick_interval_secs(), 10)
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_build_max_message_retry_times() {
    let engine = Engine::builder()
        .max_message_retry_times(100)
        .build()
        .start()
        .unwrap();
    assert_eq!(engine.config().max_message_retry_times(), 100)
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_drop() {
    let engine = Engine::new().start().unwrap();
    drop(engine);
    let engine = Engine::new().start().unwrap();
    drop(engine)
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_build_config_default() {
    if !std::path::Path::new("test").exists() {
        std::fs::create_dir("test").unwrap();
    }
    let path = "test/acts.toml";
    if std::path::Path::new(path).exists() {
        std::fs::remove_file(path).unwrap();
    }
    std::fs::write(
        path,
        r#"
        cache_cap =  100
        tick_interval_secs = 200

        [log]
        dir = "data"
        level = "INFO"
        "#,
    )
    .unwrap();
    let engine = Engine::builder().build();
    assert_eq!(engine.config().cache_cap(), 100);
    assert_eq!(engine.config().log().dir, "data");
    assert_eq!(engine.config().log().level, "INFO");
    assert_eq!(engine.config().tick_interval_secs(), 200);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_build_config_set_source() {
    if !std::path::Path::new("test").exists() {
        let _ = std::fs::create_dir("test");
    }
    let path = std::path::Path::new("test/test.toml");

    if path.exists() {
        std::fs::remove_file(path).unwrap();
    }
    std::fs::write(
        path,
        r#"
        cache_cap =  100
        tick_interval_secs = 200
        default_outputs = [ 
            "data"
        ]

        [log]
        dir = "data"
        level = "INFO"
        "#,
    )
    .unwrap();
    let engine = Engine::builder().set_config_source(path).build();
    assert_eq!(engine.config().cache_cap(), 100);
    assert_eq!(engine.config().log().dir, "data");
    assert_eq!(engine.config().log().level, "INFO");
    assert_eq!(engine.config().tick_interval_secs(), 200);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_get_custom_config() {
    #[derive(Deserialize)]
    struct Custom {
        myint: i32,
        mystr: String,
        my_option: Option<i32>,
    }

    let path = "test/acts.toml";
    if std::path::Path::new(path).exists() {
        std::fs::remove_file(path).unwrap();
    }
    std::fs::write(
        path,
        r#"
        [custom]
        myint = 100
        mystr = "myData"
        "#,
    )
    .unwrap();
    let engine = Engine::builder().build();
    let custom = engine.config().get::<Custom>("custom").unwrap();
    assert_eq!(custom.myint, 100);
    assert_eq!(custom.mystr, "myData");
    assert_eq!(custom.my_option, None);
}
