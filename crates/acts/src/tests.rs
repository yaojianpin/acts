use crate::Config;
use crate::event::EventAction;
use crate::{
    ActPackage, ActPackageCatalog, ActPackageDefinition, ActRunAs, Context, Engine, KvStore,
    MemoryStore, MessageState, ScanOperation, ScanOptions, Vars, Workflow, utils,
    utils::test::USES_IRQ,
};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::time::{Duration, timeout};

/// A package that panics after engine startup. It verifies that user-code bugs
/// cannot kill the scheduler's only queue consumer.
#[derive(Debug, Clone)]
struct PanicPackage;

#[async_trait::async_trait]
impl ActPackage for PanicPackage {
    fn definition() -> ActPackageDefinition {
        ActPackageDefinition {
            id: "test.scheduler.panic",
            name: "Panic",
            desc: "panic after scheduler startup",
            icon: "",
            doc: "",
            version: "0.1.0",
            schema: json!({}),
            options: None,
            run_as: ActRunAs::Func,
            resources: Vec::new(),
            catalog: ActPackageCatalog::App,
        }
    }

    fn new(_: &Config) -> crate::Result<Self> {
        Ok(Self)
    }

    async fn execute(
        &self,
        _ctx: &Context,
        _params: &serde_json::Value,
    ) -> crate::Result<Option<Vars>> {
        panic!("injected scheduler panic");
    }
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn scheduler_event_loop_survives_task_panic() {
    let engine = Engine::builder()
        .add_package::<PanicPackage>()
        .start()
        .await
        .unwrap();

    // Queue and process a failing task. The panic hook will print, but the
    // task must be reported as an error instead of silently killing the loop.
    let (error_tx, error_rx) = engine.signal(()).double();
    let error_signal = error_tx.clone();
    engine.channel().on_error(move |_| {
        let error_signal = error_signal.clone();
        async move { error_signal.close() }
    });
    let panic_workflow = Workflow::new().with_id("panic-model").with_step(|step| {
        step.with_id("panic-step")
            .with_uses("test.scheduler.panic", Vars::new())
    });
    engine
        .runtime()
        .start(&panic_workflow, Vars::new())
        .await
        .unwrap();
    timeout(Duration::from_secs(10), error_rx.recv())
        .await
        .expect("a panicked task must take the ordinary error path");

    // A process launched after the panic must still run to completion.
    let (tx, rx) = engine.signal(()).double();
    let s = tx.clone();
    engine.channel().on_complete(move |_| {
        let s = s.clone();
        async move { s.close() }
    });
    let healthy_workflow = Workflow::new()
        .with_id("healthy-model")
        .with_step(|step| step.with_id("healthy-step"));
    engine
        .runtime()
        .start(&healthy_workflow, Vars::new())
        .await
        .unwrap();

    timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("the scheduler must remain alive after a task panic");
    engine.close().await;
}

use serial_test::serial;

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_start() {
    let engine = Engine::builder().start().await;
    assert!(engine.is_ok());
}

/// Plugin whose `on_init` can fail on demand — used to exercise the
/// engine-start failure path.
#[derive(Clone)]
struct FailPlugin {
    fail: bool,
}

#[async_trait::async_trait]
impl crate::ActPlugin for FailPlugin {
    fn on_init(&self, _engine: &Engine) -> crate::Result<()> {
        if self.fail {
            return Err(crate::ActError::Action(
                "injected plugin init failure".to_string(),
            ));
        }
        Ok(())
    }
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_start_failure_releases_runtime_resources() {
    // a failing init makes `start()` return Err...
    let started = Engine::builder()
        .add_plugin(&FailPlugin { fail: true })
        .start()
        .await;
    let err = match started {
        Err(err) => err,
        Ok(_) => panic!("start with a failing plugin must fail"),
    };
    assert!(
        err.to_string().contains("injected"),
        "expected the injected plugin failure, got: {err}"
    );

    // ...and the partially started runtime (store writer thread, event loop)
    // must have been released: a healthy engine can still start right after
    // without deadlocking or accumulating leaked timers/threads.
    let engine = Engine::builder()
        .add_plugin(&FailPlugin { fail: false })
        .start()
        .await
        .unwrap();
    engine.close().await;
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_event_on_message() {
    let engine = Engine::builder().start().await.unwrap();
    let sig = engine.signal("".to_string());
    let s = sig.clone();
    let mid = utils::longid();
    let workflow = Workflow::new()
        .with_id(&mid)
        .with_step(|step| step.with_uses(USES_IRQ, Vars::new().with("key", "test")));

    engine.channel().on_message(move |e| {
        let s = s.clone();
        async move {
            if e.is_type("act") {
                s.update(|data| *data = e.params().unwrap().get::<String>("key").unwrap());
                s.close();
            }
        }
    });

    let executor = engine.executor();
    engine
        .executor()
        .model()
        .deploy(&workflow, None)
        .await
        .unwrap();

    let mut options = Vars::new();
    options.insert("pid".to_string(), json!(utils::longid()));
    executor.proc().start(&workflow.id, options).await.unwrap();
    let ret = sig.recv().await;
    assert_eq!(ret, "test");
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_event_on_start() {
    let engine = Engine::builder().start().await.unwrap();

    let sig = engine.signal("".to_string());
    let s = sig.clone();
    let mid = utils::longid();
    let workflow = Workflow::new()
        .with_id(&mid)
        .with_step(|step| step.with_uses(USES_IRQ, Vars::new().with("key", "test")));

    engine.channel().on_start(move |e| {
        let s = s.clone();
        async move {
            s.send(e.mid.clone());
        }
    });

    let executor = engine.executor();
    engine
        .executor()
        .model()
        .deploy(&workflow, None)
        .await
        .unwrap();

    let mut options = Vars::new();
    options.insert("pid".to_string(), json!(utils::longid()));
    executor.proc().start(&workflow.id, options).await.unwrap();
    let ret = sig.recv().await;
    assert_eq!(ret, mid);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_event_on_complete() {
    let engine = Engine::builder().start().await.unwrap();
    let sig = engine.signal(false);
    let s1 = sig.clone();
    let mid = utils::longid();
    let workflow = Workflow::new()
        .with_id(&mid)
        .with_step(|step| step.with_id("step1"));

    engine.channel().on_complete(move |e| {
        let s1 = s1.clone();
        let mid = mid.clone();
        async move {
            s1.send(e.mid == mid);
        }
    });

    let executor = engine.executor();
    engine
        .executor()
        .model()
        .deploy(&workflow, None)
        .await
        .unwrap();

    let mut options = Vars::new();
    options.insert("pid".to_string(), json!(utils::longid()));
    executor.proc().start(&workflow.id, options).await.unwrap();
    let ret = sig.recv().await;
    assert!(ret);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_event_on_error() {
    let engine = Engine::builder().start().await.unwrap();
    let mid = utils::longid();
    let workflow = Workflow::new().with_id(&mid).with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });

    let sig = engine.signal(false);
    let s1 = sig.clone();
    engine.channel().on_error(move |e| {
        let s1 = s1.clone();
        let mid = mid.clone();
        async move {
            s1.send(e.mid == mid);
        }
    });

    let rt = engine.runtime();
    engine.channel().on_message(move |e| {
        let rt = rt.clone();
        async move {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.set("ecode", "err1");

            if e.params().unwrap().get::<String>("key").as_deref() == Some("act1")
                && e.is_state(MessageState::Created)
            {
                rt.do_action2(&e.pid, &e.tid, EventAction::Error, options)
                    .await
                    .unwrap();
            }
        }
    });

    let executor = engine.executor();
    executor.model().deploy(&workflow, None).await.unwrap();

    let mut options = Vars::new();
    options.insert("pid".to_string(), json!(utils::longid()));
    executor.proc().start(&workflow.id, options).await.unwrap();
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
    let engine = Engine::builder().cache_size(100).start().await.unwrap();
    assert_eq!(engine.config().cache_cap(), 100)
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_build_log_dir() {
    let engine = Engine::builder().log("test", "INFO").start().await.unwrap();
    assert_eq!(engine.config().log().dir, "test")
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_build_log_level() {
    let engine = Engine::builder().log("log", "DEBUG").start().await.unwrap();
    assert_eq!(engine.config().log().level, "DEBUG")
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_build_tick_interval_secs() {
    let engine = Engine::builder()
        .tick_interval_secs(10)
        .start()
        .await
        .unwrap();
    assert_eq!(engine.config().tick_interval_secs(), 10)
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_build_max_message_retry_times() {
    let engine = Engine::builder()
        .max_message_retry_times(100)
        .start()
        .await
        .unwrap();
    assert_eq!(engine.config().max_message_retry_times(), 100)
}
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_build_max_node_run_times() {
    let engine = Engine::builder()
        .max_node_run_times(100)
        .start()
        .await
        .unwrap();
    assert_eq!(engine.config().max_node_run_times(), 100)
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_drop() {
    let engine = Engine::builder().start().await.unwrap();
    drop(engine);
    let engine = Engine::builder().start().await.unwrap();
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
    let builder = Engine::builder();
    let config = builder.config();
    assert_eq!(config.cache_cap(), 100);
    assert_eq!(config.log().dir, "data");
    assert_eq!(config.log().level, "INFO");
    assert_eq!(config.tick_interval_secs(), 200);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_build_config_set_config() {
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

    let config = Config::create(path).unwrap();
    let config = Engine::builder().set_config(&config).config();
    assert_eq!(config.cache_cap(), 100);
    assert_eq!(config.log().dir, "data");
    assert_eq!(config.log().level, "INFO");
    assert_eq!(config.tick_interval_secs(), 200);
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
    let config = Engine::builder().set_config_source(path).unwrap().config();
    assert_eq!(config.cache_cap(), 100);
    assert_eq!(config.log().dir, "data");
    assert_eq!(config.log().level, "INFO");
    assert_eq!(config.tick_interval_secs(), 200);
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
    let config = Engine::builder().config();
    let custom = config.get::<Custom>("custom").unwrap();
    assert_eq!(custom.myint, 100);
    assert_eq!(custom.mystr, "myData");
    assert_eq!(custom.my_option, None);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn snapshot_injects_sealed_data() {
    let engine = Engine::builder().start().await.unwrap();
    engine.add_snapshot("profile", crate::SnapshotOptions::per_proc());
    engine.snapshot().upsert(
        "profile",
        "",
        1,
        Vars::new()
            .with("secrets", Vars::new().with("TOKEN", "abc123"))
            .with("vars", Vars::new().with("DB_HOST", "10.0.0.1"))
            .with("permissions", vec!["deploy", "read_logs"]),
    );

    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "test"))
    });

    let sig = engine.signal(());
    let s1 = sig.clone();

    engine.channel().on_message(move |e| {
        let s1 = s1.clone();
        async move {
            if e.is_irq() {
                s1.close();
            }
        }
    });

    let proc = engine
        .runtime()
        .start(&workflow, Vars::new().with("unit", "u1"))
        .await
        .unwrap();

    sig.recv().await;

    // config should be in root task sealed_data
    let root = proc.root().unwrap();
    let profile = root.sealed("profile").unwrap();
    let secrets = profile.get::<Vars>("secrets").unwrap();
    assert_eq!(secrets.get::<String>("TOKEN").unwrap(), "abc123");
    let vars = profile.get::<Vars>("vars").unwrap();
    assert_eq!(vars.get::<String>("DB_HOST").unwrap(), "10.0.0.1");
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sealed_data_js_dollar_profile_access() {
    let engine = Engine::builder().start().await.unwrap();
    engine.add_snapshot("profile", crate::SnapshotOptions::per_task());
    engine.snapshot().upsert(
        "profile",
        "",
        1,
        Vars::new()
            .with("permissions", vec!["deploy", "read_logs"])
            .with("secrets", Vars::new().with("TOKEN", "sk-123")),
    );

    let env = engine.runtime().env().clone();
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });

    let sig = engine.signal(());
    let s1 = sig.clone();
    engine.channel().on_message(move |e| {
        let s1 = s1.clone();
        async move {
            if e.is_irq() {
                s1.close();
            }
        }
    });

    let proc = engine
        .runtime()
        .start(&workflow, Vars::new().with("unit", "u1"))
        .await
        .unwrap();

    sig.recv().await;

    let task = proc.task_by_params("key", "act1").last().cloned().unwrap();
    let context = task.create_context();
    Context::scope(&context, || {
        // test $profile.permissions (array access)
        let result = env.eval::<Vec<String>>("$profile.permissions").unwrap();
        assert_eq!(result, vec!["deploy".to_string(), "read_logs".to_string()]);

        // test $profile.secrets.TOKEN (nested object access)
        let token = env.eval::<String>("$profile.secrets.TOKEN").unwrap();
        assert_eq!(token, "sk-123");

        // test $profile is read-only (frozen — assignment throws)
        let err = env.eval::<serde_json::Value>("$profile.newProp = 1; $profile.newProp");
        assert!(err.is_err(), "frozen object should reject writes");
    });
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn snapshot_skips_when_scope_params_missing() {
    let engine = Engine::builder().start().await.unwrap();
    engine.add_snapshot(
        "profile",
        crate::SnapshotOptions {
            scope: vec!["unit".into(), "project".into()],
            ..Default::default()
        },
    );
    engine.snapshot().upsert(
        "profile",
        "u1/p1",
        1,
        Vars::new().with("result", "not reachable without params"),
    );

    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "test"))
    });

    let sig = engine.signal(());
    let s1 = sig.clone();
    engine.channel().on_message(move |e| {
        let s1 = s1.clone();
        async move {
            if e.is_irq() {
                s1.close();
            }
        }
    });

    // start WITHOUT required params: the scope key cannot be derived, so
    // nothing is sealed (Skip)
    let proc = engine
        .runtime()
        .start(&workflow, Vars::new())
        .await
        .unwrap();

    sig.recv().await;

    let root = proc.root().unwrap();
    assert!(!root.has_sealed());
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn snapshot_sealed_data_inherits_from_parent() {
    let engine = Engine::builder().start().await.unwrap();
    engine.add_snapshot("profile", crate::SnapshotOptions::per_proc());
    engine
        .snapshot()
        .upsert("profile", "", 1, Vars::new().with("scope", "workflow"));

    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });

    let sig = engine.signal(());
    let s1 = sig.clone();
    engine.channel().on_message(move |e| {
        let s1 = s1.clone();
        async move {
            if e.is_irq() {
                s1.close();
            }
        }
    });

    let proc = engine
        .runtime()
        .start(&workflow, Vars::new().with("unit", "u1"))
        .await
        .unwrap();

    sig.recv().await;

    // root task has sealed data
    let root = proc.root().unwrap();
    let root_profile = root.sealed("profile").unwrap();
    assert_eq!(root_profile.get::<String>("scope").unwrap(), "workflow");

    // child task inherits sealed data from parent
    let child = proc.task_by_params("key", "act1").last().cloned().unwrap();
    let child_profile = child.sealed("profile").unwrap();
    assert_eq!(child_profile.get::<String>("scope").unwrap(), "workflow");
}
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_default_store_is_memory() {
    let engine = Engine::builder().start().await.unwrap();
    let store = engine.runtime().store();
    assert!(
        store
            .procs()
            .query(&crate::query::Query::new())
            .await
            .is_ok()
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_set_store_memory() {
    let engine = Engine::builder()
        .set_store(Arc::new(MemoryStore::new()))
        .start()
        .await
        .unwrap();
    let store = engine.runtime().store();
    assert!(
        store
            .procs()
            .query(&crate::query::Query::new())
            .await
            .is_ok()
    );
}

/// Custom `KvStore` implementation injected via [`EngineBuilder::set_store`].
#[derive(Debug, Default)]
struct CustomStore {
    data: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
}

impl CustomStore {
    fn new() -> Self {
        Self::default()
    }
}

/// Return true if `k` matches the scan operation given `key` and `prefix`.
fn key_matches(k: &str, key: &str, prefix: &str, op: &ScanOperation) -> bool {
    if !k.starts_with(prefix) {
        return false;
    }
    match op {
        ScanOperation::Eq => k.starts_with(key),
        ScanOperation::Ne => !k.starts_with(key),
        ScanOperation::In { values } => values.iter().any(|v| k.starts_with(v.as_str())),
        ScanOperation::Range { lower, upper } => {
            if let Some(l) = lower
                && k < l.as_str()
            {
                return false;
            }
            if let Some(u) = upper
                && k >= u.as_str()
            {
                return false;
            }
            true
        }
    }
}

#[async_trait::async_trait]
impl KvStore for CustomStore {
    async fn get(&self, key: &str) -> crate::Result<Option<Vec<u8>>> {
        Ok(self.data.lock().get(key).cloned())
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> crate::Result<()> {
        self.data.lock().insert(key.to_string(), value);
        Ok(())
    }

    async fn delete(&self, key: &str) -> crate::Result<()> {
        self.data.lock().remove(key);
        Ok(())
    }

    async fn scan_prefix(
        &self,
        key: &str,
        options: ScanOptions,
    ) -> crate::Result<Vec<(String, Vec<u8>)>> {
        let ScanOptions {
            is_rev,
            op,
            ref prefix,
        } = options;
        let map = self.data.lock();
        let mut entries: Vec<(String, Vec<u8>)> = map
            .range(prefix.clone()..)
            .take_while(|(k, _)| k.starts_with(prefix.as_str()))
            .filter(|(k, _)| key_matches(k, key, prefix, &op))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if is_rev {
            entries.reverse();
        }
        Ok(entries)
    }
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_set_store_custom() {
    let custom = Arc::new(CustomStore::new());
    let engine = Engine::builder()
        .set_store(custom.clone())
        .start()
        .await
        .unwrap();

    // writes through the engine must land in the custom store
    let model = Workflow::new().with_id("custom_store_model");
    engine
        .executor()
        .model()
        .deploy(&model, None)
        .await
        .unwrap();

    // reads must go through the custom store
    let store = engine.runtime().store();
    assert!(store.models().find("custom_store_model").await.is_ok());
    let page = store
        .models()
        .query(&crate::query::Query::new())
        .await
        .unwrap();
    assert_eq!(page.count, 1);

    // the raw key must physically exist in the custom store
    assert!(
        custom
            .data
            .lock()
            .keys()
            .any(|k| k.contains("custom_store_model"))
    );
}

#[test]
#[should_panic(expected = "only one backend")]
fn engine_builder_set_store_duplicate() {
    let _ = Engine::builder()
        .set_store(Arc::new(MemoryStore::new()))
        .set_store(Arc::new(MemoryStore::new()));
}
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn snapshot_per_proc_pins_value_until_process_ends() {
    let engine = Engine::builder().start().await.unwrap();
    engine.add_snapshot("profile", crate::SnapshotOptions::per_proc());

    // external system feeds v1 before the process starts
    engine
        .snapshot()
        .upsert("profile", "", 1, Vars::new().with("val", 1));

    let workflow = Workflow::new()
        .with_id("snap_proc_pin")
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new().with("key", "stop1"))
        })
        .with_step(|step| {
            step.with_id("s2")
                .with_uses(USES_IRQ, Vars::new().with("key", "stop2"))
        });

    let sig = engine.signal(String::new());
    let s = sig.clone();
    let eng = engine.clone();
    let executor = engine.executor();
    engine.channel().on_message(move |e| {
        let s = s.clone();
        let eng = eng.clone();
        let executor = executor.clone();
        async move {
            if !e.is_irq() || !e.is_state(MessageState::Created) {
                return;
            }
            let key = e.params().unwrap().get::<String>("key").unwrap_or_default();
            if key == "stop1" {
                // external system refreshes the snapshot mid-run
                eng.snapshot()
                    .upsert("profile", "", 2, Vars::new().with("val", 2));
                executor
                    .act()
                    .complete(&e.pid, &e.tid, Vars::new())
                    .await
                    .unwrap();
            } else if key == "stop2" {
                executor
                    .act()
                    .complete(&e.pid, &e.tid, Vars::new())
                    .await
                    .unwrap();
                s.update(|data| *data = "stop2".to_string());
                s.close();
            }
        }
    });

    let pid = utils::longid();
    engine
        .runtime()
        .start(&workflow, Vars::new().with("pid", pid.clone()))
        .await
        .unwrap();

    sig.recv().await;

    let proc = engine.runtime().proc(&pid).await.unwrap().unwrap();
    let root = proc.root().unwrap();
    let step2 = proc.task_by_params("key", "stop2").last().cloned().unwrap();

    // root sealed the value once at process start
    assert_eq!(
        root.sealed("profile").unwrap().get::<i32>("val").unwrap(),
        1
    );
    // the descendant did NOT re-resolve after the mid-run refresh — it
    // inherited the pinned value from its lineage
    assert!(!step2.has_sealed_local("profile"));
    assert_eq!(
        step2.sealed("profile").unwrap().get::<i32>("val").unwrap(),
        1
    );
    // the cache itself moved on; the next process will see rev 2
    assert_eq!(engine.snapshot().read("profile", "").unwrap().rev, 2);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn snapshot_per_task_reads_latest_value() {
    let engine = Engine::builder().start().await.unwrap();
    engine.add_snapshot("profile", crate::SnapshotOptions::per_task());

    engine
        .snapshot()
        .upsert("profile", "", 1, Vars::new().with("val", 1));

    let workflow = Workflow::new()
        .with_id("snap_task_latest")
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_IRQ, Vars::new().with("key", "stop1"))
        })
        .with_step(|step| {
            step.with_id("s2")
                .with_uses(USES_IRQ, Vars::new().with("key", "stop2"))
        });

    let sig = engine.signal(String::new());
    let s = sig.clone();
    let eng = engine.clone();
    let executor = engine.executor();
    engine.channel().on_message(move |e| {
        let s = s.clone();
        let eng = eng.clone();
        let executor = executor.clone();
        async move {
            if !e.is_irq() || !e.is_state(MessageState::Created) {
                return;
            }
            let key = e.params().unwrap().get::<String>("key").unwrap_or_default();
            if key == "stop1" {
                // external system refreshes the snapshot mid-run
                eng.snapshot()
                    .upsert("profile", "", 2, Vars::new().with("val", 2));
                executor
                    .act()
                    .complete(&e.pid, &e.tid, Vars::new())
                    .await
                    .unwrap();
            } else if key == "stop2" {
                executor
                    .act()
                    .complete(&e.pid, &e.tid, Vars::new())
                    .await
                    .unwrap();
                s.update(|data| *data = "stop2".to_string());
                s.close();
            }
        }
    });

    let pid = utils::longid();
    engine
        .runtime()
        .start(&workflow, Vars::new().with("pid", pid.clone()))
        .await
        .unwrap();

    sig.recv().await;

    let proc = engine.runtime().proc(&pid).await.unwrap().unwrap();
    let root = proc.root().unwrap();
    let step2 = proc.task_by_params("key", "stop2").last().cloned().unwrap();

    // every new task re-reads the cache: step2 sees the refreshed v2
    assert_eq!(
        root.sealed("profile").unwrap().get::<i32>("val").unwrap(),
        1
    );
    assert!(step2.has_sealed_local("profile"));
    assert_eq!(
        step2.sealed("profile").unwrap().get::<i32>("val").unwrap(),
        2
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn snapshot_scope_keyed_by_task_params() {
    let engine = Engine::builder().start().await.unwrap();
    engine.add_snapshot(
        "profile",
        crate::SnapshotOptions {
            scope: vec!["unit".to_string()],
            ..Default::default()
        },
    );

    engine
        .snapshot()
        .upsert("profile", "u1", 1, Vars::new().with("val", "A"));
    engine
        .snapshot()
        .upsert("profile", "u2", 1, Vars::new().with("val", "B"));

    let workflow = Workflow::new()
        .with_id("snap_scope")
        .with_step(|step| {
            step.with_id("s1")
                .with_var("unit", "u1")
                .with_uses(USES_IRQ, Vars::new().with("key", "stop1"))
        })
        .with_step(|step| {
            step.with_id("s2")
                .with_var("unit", "u2")
                .with_uses(USES_IRQ, Vars::new().with("key", "stop2"))
        });

    let sig = engine.signal(String::new());
    let s = sig.clone();
    let executor = engine.executor();
    engine.channel().on_message(move |e| {
        let s = s.clone();
        let executor = executor.clone();
        async move {
            if !e.is_irq() || !e.is_state(MessageState::Created) {
                return;
            }
            let key = e.params().unwrap().get::<String>("key").unwrap_or_default();
            if key == "stop1" {
                executor
                    .act()
                    .complete(&e.pid, &e.tid, Vars::new())
                    .await
                    .unwrap();
            } else if key == "stop2" {
                executor
                    .act()
                    .complete(&e.pid, &e.tid, Vars::new())
                    .await
                    .unwrap();
                s.update(|data| *data = "stop2".to_string());
                s.close();
            }
        }
    });

    let pid = utils::longid();
    engine
        .runtime()
        .start(&workflow, Vars::new().with("pid", pid.clone()))
        .await
        .unwrap();

    sig.recv().await;

    let proc = engine.runtime().proc(&pid).await.unwrap().unwrap();
    let step1 = proc.task_by_params("key", "stop1").last().cloned().unwrap();
    let step2 = proc.task_by_params("key", "stop2").last().cloned().unwrap();

    // each task seals the snapshot of ITS scope (unit param)
    assert_eq!(
        step1
            .sealed("profile")
            .unwrap()
            .get::<String>("val")
            .unwrap(),
        "A"
    );
    assert_eq!(
        step2
            .sealed("profile")
            .unwrap()
            .get::<String>("val")
            .unwrap(),
        "B"
    );
}
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn snapshot_per_proc_js_access_inherits_on_child() {
    // per-proc seals once on the root lineage; a child task has no local
    // sealed data, yet its JS environment must still expose $profile
    let engine = Engine::builder().start().await.unwrap();
    engine.add_snapshot("profile", crate::SnapshotOptions::per_proc());
    engine.snapshot().upsert(
        "profile",
        "",
        1,
        Vars::new()
            .with("permissions", vec!["deploy", "read_logs"])
            .with("secrets", Vars::new().with("TOKEN", "sk-123")),
    );

    let env = engine.runtime().env().clone();
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });

    let sig = engine.signal(());
    let s1 = sig.clone();
    engine.channel().on_message(move |e| {
        let s1 = s1.clone();
        async move {
            if e.is_irq() && e.is_state(MessageState::Created) {
                s1.close();
            }
        }
    });

    let proc = engine
        .runtime()
        .start(&workflow, Vars::new().with("unit", "u1"))
        .await
        .unwrap();

    sig.recv().await;

    let task = proc.task_by_params("key", "act1").last().cloned().unwrap();
    // the child itself must not carry sealed data — it inherits from root
    assert!(!task.has_sealed_local("profile"));
    let context = task.create_context();
    Context::scope(&context, || {
        let result = env.eval::<Vec<String>>("$profile.permissions").unwrap();
        assert_eq!(result, vec!["deploy".to_string(), "read_logs".to_string()]);
        let token = env.eval::<String>("$profile.secrets.TOKEN").unwrap();
        assert_eq!(token, "sk-123");
    });
}
