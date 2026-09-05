use crate::event::EventAction;
use crate::{Config, ConfigResolver};
use crate::{
    Context, Engine, KvStore, MemoryStore, MessageState, ScanOperation, ScanOptions, Vars,
    Workflow, utils, utils::test::USES_IRQ,
};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;

use serial_test::serial;

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_start() {
    let engine = Engine::new().start().await;
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
        .build()
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
        .build()
        .start()
        .await
        .unwrap();
    engine.close().await;
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_event_on_message() {
    let engine = Engine::new().start().await.unwrap();
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
    let engine = Engine::new().start().await.unwrap();

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
    let engine = Engine::new().start().await.unwrap();
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
    let engine = Engine::new().start().await.unwrap();
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
    let engine = Engine::builder()
        .cache_size(100)
        .build()
        .start()
        .await
        .unwrap();
    assert_eq!(engine.config().cache_cap(), 100)
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_build_log_dir() {
    let engine = Engine::builder()
        .log("test", "INFO")
        .build()
        .start()
        .await
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
        .await
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
        .await
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
        .await
        .unwrap();
    assert_eq!(engine.config().max_message_retry_times(), 100)
}
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_build_max_node_run_times() {
    let engine = Engine::builder()
        .max_node_run_times(100)
        .build()
        .start()
        .await
        .unwrap();
    assert_eq!(engine.config().max_node_run_times(), 100)
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_drop() {
    let engine = Engine::new().start().await.unwrap();
    drop(engine);
    let engine = Engine::new().start().await.unwrap();
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

    let config = Config::create(path);
    let engine = Engine::builder().set_config(&config).build();
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

struct TestResolver {
    data: Vars,
}

#[async_trait::async_trait]
impl ConfigResolver for TestResolver {
    async fn resolve(&self, _ctx: &Vars) -> crate::Result<Vars> {
        Ok(self.data.clone())
    }
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn config_resolver_injects_sealed_data() {
    let resolver = Arc::new(TestResolver {
        data: Vars::new()
            .with("secrets", Vars::new().with("TOKEN", "abc123"))
            .with("vars", Vars::new().with("DB_HOST", "10.0.0.1"))
            .with("permissions", vec!["deploy", "read_logs"]),
    });

    let engine = Engine::new().start().await.unwrap();
    engine.add_resolver("profile", resolver);

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
    let resolver = Arc::new(TestResolver {
        data: Vars::new()
            .with("permissions", vec!["deploy", "read_logs"])
            .with("secrets", Vars::new().with("TOKEN", "sk-123")),
    });

    let engine = Engine::new().start().await.unwrap();
    engine.add_resolver("profile", resolver);

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
    Context::scope(context, || {
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
async fn config_resolver_skips_when_required_params_missing() {
    struct StrictResolver;

    #[async_trait::async_trait]
    impl ConfigResolver for StrictResolver {
        fn required_params(&self) -> Vec<String> {
            vec!["unit".into(), "project".into()]
        }

        async fn resolve(&self, _ctx: &Vars) -> crate::Result<Vars> {
            Ok(Vars::new().with("result", "should not be called"))
        }
    }

    let engine = Engine::new().start().await.unwrap();
    engine.add_resolver("profile", Arc::new(StrictResolver));

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

    // start WITHOUT required params
    let proc = engine
        .runtime()
        .start(&workflow, Vars::new())
        .await
        .unwrap();

    sig.recv().await;

    let root = proc.root().unwrap();
    // sealed_data should be empty since required params were missing
    assert!(!root.has_sealed());
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sealed_data_inherits_from_parent() {
    let resolver = Arc::new(TestResolver {
        data: Vars::new().with("scope", "workflow"),
    });

    let engine = Engine::new().start().await.unwrap();
    engine.add_resolver("profile", resolver);

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
    let engine = Engine::new().start().await.unwrap();
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
        .build()
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

#[cfg(feature = "store-sqlite")]
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_set_store_sqlite() {
    let store = crate::store::SqliteStore::open(":memory:").await.unwrap();
    let engine = Engine::builder()
        .set_store(Arc::new(store))
        .build()
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
        .build()
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

/// Regression: an engine backed by a real (async-driver) store must work on a
/// current-thread tokio runtime. Before the async store migration every
/// async-context store op went through `tokio::task::block_in_place`, which
/// panics on current-thread runtimes — the retry/schedule timers (and every
/// task transition) hit that on their first tick.
#[cfg(feature = "store-sqlite")]
#[serial]
#[tokio::test]
async fn engine_sqlite_runs_on_current_thread_runtime() {
    let store = crate::store::SqliteStore::open_in_memory().await.unwrap();
    let engine = Engine::builder()
        .set_store(Arc::new(store))
        .tick_interval_secs(1)
        .build()
        .start()
        .await
        .unwrap();

    let model = Workflow::new()
        .with_id("current_thread_model")
        .with_step(|step| {
            step.with_id("step1")
                .with_uses("acts.transform.set", Vars::new().with("a", 1))
        });

    let (done, sig) = engine.signal(bool::default()).double();
    engine
        .executor()
        .model()
        .deploy(&model, None)
        .await
        .unwrap();
    engine.channel().on_complete(move |e| {
        let done = done.clone();
        async move {
            if e.mid == "current_thread_model" {
                done.send(true);
            }
        }
    });
    engine
        .executor()
        .proc()
        .start("current_thread_model", Vars::new())
        .await
        .unwrap();

    // long enough for the retry timer to tick (test builds tick every 800ms)
    let ret = sig.timeout(4000).await;
    assert!(ret, "workflow did not complete on a current-thread runtime");
    engine.close().await;
}
