use crate::Config;
use crate::event::EventAction;
use crate::{
    ActPackage, ActPackageCatalog, ActPackageDefinition, ActRunAs, Context, Engine, KvStore,
    MemoryStore, MessageState, ScanOperation, ScanOptions, Vars, Workflow, utils,
    utils::test::USES_IRQ, utils::test::USES_SET,
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

/// Plugin that spawns a task from `on_init` and stops it on the engine
/// shutdown token — the pattern the transport plugins (gRPC, web, NATS) use.
#[derive(Clone)]
struct ShutdownTaskPlugin {
    stopped: crate::Signal<bool>,
}

#[async_trait::async_trait]
impl crate::ActPlugin for ShutdownTaskPlugin {
    fn on_init(&self, engine: &Engine) -> crate::Result<()> {
        let shutdown = engine.shutdown_token();
        let stopped = self.stopped.clone();
        tokio::spawn(async move {
            shutdown.cancelled().await;
            stopped.close();
        });
        Ok(())
    }
}

/// `Engine::close` must fire the shutdown token: a plugin task that outlives
/// the engine would otherwise keep the process alive (or be force-killed with
/// unflushed work) when the server shuts down.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn engine_close_fires_shutdown_token() {
    let stopped = crate::Signal::new(false);
    let rx = stopped.clone();
    let engine = Engine::builder()
        .add_plugin(&ShutdownTaskPlugin { stopped })
        .start()
        .await
        .unwrap();

    assert!(
        !rx.data(),
        "the plugin task must not stop before the engine closes"
    );

    engine.close().await;
    timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("engine close must fire the shutdown token");
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

    let executor = engine.executor(&crate::Principal::unrestricted());
    engine
        .executor(&crate::Principal::unrestricted())
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

    let executor = engine.executor(&crate::Principal::unrestricted());
    engine
        .executor(&crate::Principal::unrestricted())
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
    let sig = engine.signal((String::new(), 0i64, 0i64, 0i64));
    let s1 = sig.clone();
    let mid = utils::longid();
    let workflow = Workflow::new()
        .with_id(&mid)
        .with_step(|step| step.with_id("step1"));

    engine.channel().on_complete(move |e| {
        let s1 = s1.clone();
        async move {
            s1.send((e.mid.clone(), e.start_time, e.end_time, e.cost()));
        }
    });

    let executor = engine.executor(&crate::Principal::unrestricted());
    engine
        .executor(&crate::Principal::unrestricted())
        .model()
        .deploy(&workflow, None)
        .await
        .unwrap();

    let mut options = Vars::new();
    options.insert("pid".to_string(), json!(utils::longid()));
    executor.proc().start(&workflow.id, options).await.unwrap();
    let (completed_mid, start_time, end_time, cost) = sig.recv().await;
    assert_eq!(completed_mid, mid);
    // The completion event is the run's lifecycle message: its window is the
    // projected proc row's, so `cost()` reads the real run duration instead of
    // subtracting two zeroes.
    assert!(start_time > 0, "completion message must carry start_time");
    assert!(
        end_time >= start_time,
        "completion message end_time {end_time} precedes start_time {start_time}"
    );
    assert_eq!(
        cost,
        end_time - start_time,
        "cost must be the end-to-start window, not 0"
    );
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

    let executor = engine.executor(&crate::Principal::unrestricted());
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

#[tokio::test(flavor = "multi_thread")]
async fn engine_start_rejects_max_message_retry_times_below_one() {
    for times in [0, -1] {
        let err = Engine::builder()
            .max_message_retry_times(times)
            .start()
            .await
            .err()
            .expect("start must refuse a retry budget below 1");
        assert!(
            matches!(&err, crate::ActError::Config(msg) if msg.contains("max_message_retry_times")),
            "{err:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn engine_start_rejects_max_node_run_times_below_one() {
    for times in [0, -1] {
        let err = Engine::builder()
            .max_node_run_times(times)
            .start()
            .await
            .err()
            .expect("start must refuse a node-run guard below 1");
        assert!(
            matches!(&err, crate::ActError::Config(msg) if msg.contains("max_node_run_times")),
            "{err:?}"
        );
    }
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
    async fn one(&self, key: &str) -> crate::Result<Option<Vec<u8>>> {
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
        .executor(&crate::Principal::unrestricted())
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

/// A run whose authority does not own the scope its act wants to seal fails
/// instead of waiting forever.
///
/// The start operation resolves the act's scope first; a seal refusal there is
/// handed to the act's own operation (the state table, like the legacy sequence,
/// only decides a failure from a started act). `acts.core.irq` waits for a
/// client and never executes a package, so that operation must prepare the scope
/// anyway — otherwise the refusal has no operation left to land on and the run
/// stays `Running` with no failure and no message (the wedge this pins).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_run_that_cannot_seal_fails_instead_of_waiting() {
    let engine = Engine::builder().start().await.unwrap();
    engine
        .add_snapshot("profile", crate::SnapshotOptions::per_proc())
        .unwrap();
    engine
        .snapshot()
        .upsert("profile", "", 1, Vars::new().with("val", 1))
        .unwrap();

    let workflow = Workflow::new().with_id("seal-denied").with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "test"))
    });
    let pid = utils::longid();
    let started = engine
        .runtime()
        .start(&workflow, Vars::new().with("pid", &pid))
        .await
        .unwrap();
    assert_eq!(started, pid);

    // The run settles (no client ever answers) and the failing act carries
    // the refusal.
    let mut settled = false;
    for _ in 0..200 {
        let run = engine.runtime().read_proc(&pid).await.unwrap();
        if let Some(run) = run
            && run.state().is_error()
        {
            settled = true;
            // The failing act carries the refusal, and so does every ancestor
            // the error walk decided — the legacy `parent.set_err` wrote the
            // same code/message up the chain, so the run's own error names what
            // failed.
            for (tid, err) in run
                .tasks()
                .iter()
                .map(|task| (task.tid().to_string(), task.err()))
                .collect::<Vec<_>>()
            {
                assert!(
                    err.clone()
                        .is_some_and(|err| err.message.contains("is not owned by subject")),
                    "task {tid} must carry the refusal: {err:?}"
                );
            }
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        settled,
        "the refused seal must fail the run, not leave it waiting"
    );
    // The cache still holds the value: it was the authority that did not.
    assert_eq!(engine.snapshot().read("profile", "").unwrap().rev, 1);
    engine.close().await;
}
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn snapshot_rejects_unrepresentable_ttl() {
    // builder path: start() validates and fails loudly
    let err = Engine::builder()
        .add_snapshot(
            "profile",
            crate::SnapshotOptions::default().with_ttl(u64::MAX),
        )
        .start()
        .await
        .map(|_| ())
        .unwrap_err();
    assert!(matches!(err, crate::ActError::Config(_)), "got: {err}");

    // runtime path, plus the exact boundary accepted
    let engine = Engine::builder().start().await.unwrap();
    for ttl in [u64::MAX, crate::MAX_TTL_SECS + 1] {
        let err = engine
            .add_snapshot("profile", crate::SnapshotOptions::default().with_ttl(ttl))
            .unwrap_err();
        assert!(
            matches!(err, crate::ActError::Config(_)),
            "ttl {ttl}: {err}"
        );
    }
    engine
        .add_snapshot(
            "profile",
            crate::SnapshotOptions::default().with_ttl(crate::MAX_TTL_SECS),
        )
        .unwrap();
    engine.close().await;
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn snapshot_manager_remove_requires_registration() {
    let engine = Engine::builder().start().await.unwrap();

    // upsert auto-registers the target, so the tombstone applies
    engine
        .snapshot()
        .upsert("auto", "", 1, Vars::new().with("a", 1))
        .unwrap();
    engine.snapshot().remove("auto", "").unwrap();
    assert!(engine.snapshot().read("auto", "").is_none());

    // a target the engine never registered is a wiring error, not a no-op
    let err = engine.snapshot().remove("missing", "").unwrap_err();
    assert!(matches!(err, crate::ActError::Runtime(_)), "got: {err}");
    assert!(err.to_string().contains("missing"), "got: {err}");

    engine.close().await;
}

// ---------------------------------------------------------------------------
// Run (message) semantics — the surface the published example models read
// ---------------------------------------------------------------------------

/// A run completes with exactly its model's `exposes`: the declaration is the
/// run's output *surface*, not a schema for every variable the run happens to
/// hold.
///
/// The whole root scope used to be validated against the exposes schema, whose
/// `additionalProperties: false` rejected every name the model did not declare
/// — so a model with bookkeeping variables beside one exposed result ended as
/// an *errored* run (`model(...) outputs validation error: Additional
/// properties are not allowed ('index' was unexpected)`), which is what
/// `examples/simple`, `examples/while`, `examples/custom_package` and
/// `examples/subflow` all announced.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_run_completes_with_its_exposed_outputs() {
    let engine = Engine::builder().start().await.unwrap();
    let workflow = Workflow::from_yml(
        r#"
id: exposes-projection
ver: 0.1.0
vars:
  - name: index
    value: 0
  - name: value
    value: 0
exposes:
  - name: result
    value: ${{ value }}
steps:
  - name: bookkeeping
    id: bookkeeping
    uses: acts.transform.set
    params:
      index: 7
  - name: accumulate
    id: accumulate
    uses: acts.transform.set
    params:
      value: ${{ index + 48 }}
"#,
    )
    .unwrap();
    engine
        .executor(&crate::Principal::unrestricted())
        .model()
        .deploy(&workflow, None)
        .await
        .unwrap();

    let (s, sig) = engine.signal(json!(null)).double();
    engine.channel().on_complete(move |e| {
        let s = s.clone();
        async move {
            s.send(e.outputs.to_value());
        }
    });
    engine.channel().on_error(move |e| async move {
        panic!("the run must not error: {:?}", e.inputs);
    });

    engine
        .executor(&crate::Principal::unrestricted())
        .proc()
        .start(&workflow.id, Vars::new())
        .await
        .unwrap();

    let outputs = timeout(Duration::from_secs(20), sig.recv())
        .await
        .expect("the run never completed");
    assert_eq!(
        outputs,
        json!({ "result": 55 }),
        "only the declared expose is an output, and it answers its own value"
    );
    engine.close().await;
}

/// The published `examples/subflow` model: the parent subflow act waits for
/// the child run, folds the child's outputs into the parent scope, and only
/// then lets the parent's `final` act run. The observable order is the one a
/// channel subscriber sees — the engine's own events, not a poll of rows.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn subflow_waits_for_its_child_before_the_parent_continues() {
    let engine = Engine::builder().start().await.unwrap();
    let executor = engine.executor(&crate::Principal::unrestricted());
    for model in [
        include_str!("../../../examples/subflow/model/main.yml"),
        include_str!("../../../examples/subflow/model/sub.yml"),
    ] {
        let workflow = Workflow::from_yml(model).unwrap();
        executor.model().deploy(&workflow, None).await.unwrap();
    }

    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let log = seen.clone();
    engine.channel().on_start({
        let log = log.clone();
        move |e| {
            let log = log.clone();
            async move {
                log.lock().push(format!("{}/started", e.mid));
            }
        }
    });

    let (done, sig) = engine.signal(json!(null)).double();
    engine.channel().on_complete({
        let log = log.clone();
        move |e| {
            let log = log.clone();
            let done = done.clone();
            async move {
                log.lock().push(format!("{}/completed", e.mid));
                if e.mid == "main" {
                    done.send(e.outputs.to_value());
                }
            }
        }
    });
    engine.channel().on_error(move |e| async move {
        panic!("the subflow example must not error: {:?}", e.inputs);
    });

    let ex = executor.clone();
    engine.channel().on_message({
        let log = log.clone();
        move |e| {
            let log = log.clone();
            let ex = ex.clone();
            async move {
                if !e.is_type("act") || !e.is_state(MessageState::Created) {
                    return;
                }
                log.lock().push(format!("{}/{}", e.mid, e.name));
                let Some(params) = e.params() else {
                    return;
                };
                let key = params.get::<String>("key").unwrap_or_default();
                let mut outputs = Vars::new();
                match key.as_str() {
                    "init" => outputs.set("v", 10),
                    "action1" => outputs.set("v", 100),
                    "action2" => outputs.set(
                        "v",
                        params.get::<i64>("v").unwrap_or_default().saturating_mul(2),
                    ),
                    _ => return,
                }
                ex.act().complete(&e.pid, &e.tid, outputs).await.unwrap();
            }
        }
    });

    executor.proc().start("main", Vars::new()).await.unwrap();

    let outputs = timeout(Duration::from_secs(20), sig.recv())
        .await
        .expect("the parent run never completed");
    assert_eq!(
        outputs,
        json!({ "result": 200 }),
        "the child's v=100 folds into the parent before final doubles it"
    );

    // The completion event can reach this test before the last act message
    // callback runs; wait for the log itself before ordering it.
    timeout(Duration::from_secs(20), async {
        loop {
            if seen.lock().iter().any(|entry| entry == "main/completed") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the parent completion was never observed on the channel");

    let log = seen.lock().clone();
    let pos = |label: &str| {
        log.iter()
            .position(|entry| entry == label)
            .unwrap_or_else(|| panic!("missing '{label}' in {log:?}"))
    };
    let order = [
        "main/started",
        "main/prepare",
        "sub/started",
        "sub/t1",
        "sub/completed",
        "main/final",
        "main/completed",
    ];
    let mut previous = pos(order[0]);
    for label in &order[1..] {
        let current = pos(label);
        assert!(
            current > previous,
            "'{label}' must follow the previous event: {log:?}"
        );
        previous = current;
    }
    engine.close().await;
}

/// A model that declares no `exposes` outputs its own variables (the legacy
/// fallback), never the engine's durable facts.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_run_without_exposes_outputs_its_own_variables() {
    let engine = Engine::builder().start().await.unwrap();
    let workflow = Workflow::new()
        .with_id("no-exposes")
        .with_var("a", 0)
        .with_step(|step| {
            step.with_id("s1")
                .with_uses(USES_SET, Vars::new().with("a", 42))
        });
    engine
        .executor(&crate::Principal::unrestricted())
        .model()
        .deploy(&workflow, None)
        .await
        .unwrap();

    let (s, sig) = engine.signal(json!(null)).double();
    engine.channel().on_complete(move |e| {
        let s = s.clone();
        async move {
            s.send(e.outputs.to_value());
        }
    });
    engine
        .executor(&crate::Principal::unrestricted())
        .proc()
        .start(&workflow.id, Vars::new())
        .await
        .unwrap();

    let outputs = timeout(Duration::from_secs(20), sig.recv())
        .await
        .expect("the run never completed");
    assert_eq!(outputs, json!({ "a": 42 }));
    engine.close().await;
}

/// A failure reports itself in the message's `outputs` — `ecode` and `message`
/// are what a consumer reads to learn why an act or a run failed — and stays in
/// its `inputs`, where the clients and the catch predicates have always read
/// them.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_failure_reports_its_ecode_in_the_messages_outputs() {
    let engine = Engine::builder().start().await.unwrap();
    let workflow = Workflow::new()
        .with_id("failure-outputs")
        .with_step(|step| {
            step.with_id("step1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
        });
    engine
        .executor(&crate::Principal::unrestricted())
        .model()
        .deploy(&workflow, None)
        .await
        .unwrap();

    let (s, sig) = engine.signal(json!(null)).double();
    engine.channel().on_error(move |e| {
        let s = s.clone();
        async move {
            s.send(e.outputs.to_value());
        }
    });
    let executor = engine.executor(&crate::Principal::unrestricted());
    let ex = executor.clone();
    engine.channel().on_message(move |e| {
        let ex = ex.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                let mut vars = Vars::new();
                vars.set("ecode", "err1");
                vars.set("message", "boom");
                ex.act().fail(&e.pid, &e.tid, vars).await.unwrap();
            }
        }
    });
    executor
        .proc()
        .start(&workflow.id, Vars::new())
        .await
        .unwrap();

    let outputs = timeout(Duration::from_secs(20), sig.recv())
        .await
        .expect("the run never errored");
    assert_eq!(outputs.get("ecode"), Some(&json!("err1")));
    assert_eq!(outputs.get("message"), Some(&json!("boom")));
    engine.close().await;
}

/// A run that *recovered* from a caught failure is a successful run: the
/// engine's own failure vocabulary (`ecode`/`message`/`error.*`, which the
/// catch fan-out pins on the failed step for its handlers' guards) is not part
/// of its outputs, and the declared handler is the one that ran.
///
/// This is the published `examples/catches` model, driven to its `err1` catch.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_recovered_run_carries_no_error_facts_in_its_outputs() {
    let engine = Engine::builder().start().await.unwrap();
    let workflow = Workflow::from_yml(include_str!("../../../examples/catches/model.yml")).unwrap();
    engine
        .executor(&crate::Principal::unrestricted())
        .model()
        .deploy(&workflow, None)
        .await
        .unwrap();

    let caught = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = caught.clone();
    engine.channel().on_message(move |e| {
        let seen = seen.clone();
        async move {
            if let Some(params) = e.params()
                && let Some(key) = params.get::<String>("key")
                && (e.is_state(MessageState::Created)
                    || (e.is_msg() && e.is_state(MessageState::Completed)))
            {
                seen.lock().push(key);
            }
        }
    });
    let (s, sig) = engine.signal(json!(null)).double();
    engine.channel().on_complete(move |e| {
        let s = s.clone();
        async move {
            s.send(e.outputs.to_value());
        }
    });
    engine.channel().on_error(move |e| async move {
        panic!("a caught failure is not a failed run: {:?}", e.inputs);
    });

    let executor = engine.executor(&crate::Principal::unrestricted());
    let ex = executor.clone();
    engine.channel().on_message(move |e| {
        let ex = ex.clone();
        async move {
            match e.params().and_then(|p| p.get::<String>("key")).as_deref() {
                Some("init") if e.is_state(MessageState::Created) => {
                    ex.act()
                        .complete(&e.pid, &e.tid, Vars::new())
                        .await
                        .unwrap();
                }
                // the working act fails; the model's `err1` handler answers it
                Some("act1") if e.is_state(MessageState::Created) => {
                    let mut vars = Vars::new();
                    vars.set("ecode", "err1");
                    ex.act().fail(&e.pid, &e.tid, vars).await.unwrap();
                }
                _ => {}
            }
        }
    });
    executor
        .proc()
        .start(&workflow.id, Vars::new())
        .await
        .unwrap();

    let outputs = timeout(Duration::from_secs(20), sig.recv())
        .await
        .expect("a caught failure completes the run");
    assert_eq!(
        outputs,
        json!({}),
        "the run's outputs are its own data, never the failure vocabulary"
    );
    assert!(
        caught.lock().iter().any(|key| key == "catch1"),
        "the err1 handler must have run: {:?}",
        caught.lock()
    );
    engine.close().await;
}

// ---------------------------------------------------------------------------
// Step guards and loops — `if`/`while` and backward `next`
// ---------------------------------------------------------------------------

/// A `while` step is a bounded loop: the guard is re-read after every
/// iteration and the step re-enters while it holds, then the flow falls
/// through to the next declared step once it fails.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_while_step_loops_until_its_guard_fails() {
    let engine = Engine::builder().start().await.unwrap();
    // `examples/while`'s shape: `add` accumulates index into value while
    // `index <= input` (10), so the sum is 0+1+…+10.
    let workflow = Workflow::from_yml(include_str!("../../../examples/while/model.yml")).unwrap();
    engine
        .executor(&crate::Principal::unrestricted())
        .model()
        .deploy(&workflow, None)
        .await
        .unwrap();

    let (s, sig) = engine.signal(json!(null)).double();
    engine.channel().on_complete(move |e| {
        let s = s.clone();
        async move {
            s.send(e.outputs.to_value());
        }
    });
    engine.channel().on_error(move |e| async move {
        panic!("the loop must finish, not fail: {:?}", e.inputs);
    });
    engine
        .executor(&crate::Principal::unrestricted())
        .proc()
        .start(&workflow.id, Vars::new().with("input", 10))
        .await
        .unwrap();

    let outputs = timeout(Duration::from_secs(20), sig.recv())
        .await
        .expect("the loop never finished");
    assert_eq!(
        outputs,
        json!({ "result": 55 }),
        "the loop must run while the guard holds (0+…+10)"
    );
    engine.close().await;
}

/// A step whose `if` guard fails is *skipped* and the flow falls through to the
/// next declared step: its own `next` is not taken (which is what makes a
/// guarded self/backward `next` loop terminate), and its act never runs.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_step_whose_guard_fails_is_skipped_and_the_flow_falls_through() {
    let engine = Engine::builder().start().await.unwrap();
    let workflow = Workflow::from_yml(
        r#"
id: guarded-skip
ver: 0.1.0
vars:
  - name: hit
    value: 0
exposes:
  - name: result
    value: ${{ hit }}
steps:
  - name: guard
    id: guard
    if: "false"
    next: guard
    uses: acts.transform.set
    params:
      hit: 99
  - name: afterwards
    id: afterwards
    uses: acts.transform.set
    params:
      hit: 7
"#,
    )
    .unwrap();
    engine
        .executor(&crate::Principal::unrestricted())
        .model()
        .deploy(&workflow, None)
        .await
        .unwrap();

    let (s, sig) = engine.signal(json!(null)).double();
    engine.channel().on_complete(move |e| {
        let s = s.clone();
        async move {
            s.send(e.outputs.to_value());
        }
    });
    engine.channel().on_error(move |e| async move {
        panic!("a skipped step is not a failed run: {:?}", e.inputs);
    });
    engine
        .executor(&crate::Principal::unrestricted())
        .proc()
        .start(&workflow.id, Vars::new())
        .await
        .unwrap();

    let outputs = timeout(Duration::from_secs(20), sig.recv())
        .await
        .expect("the run never completed");
    assert_eq!(
        outputs,
        json!({ "result": 7 }),
        "the guarded step must not run and the chain must go on"
    );
    engine.close().await;
}

/// A backward `next` re-enters the node it names — the `examples/simple` loop:
/// `cond`'s branch guard is re-evaluated every time `add` jumps back into it,
/// so the loop ends when the guard fails.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_backward_next_re_enters_the_node_it_names() {
    let engine = Engine::builder().start().await.unwrap();
    let workflow = Workflow::from_yml(include_str!("../../../examples/simple/model.yml")).unwrap();
    engine
        .executor(&crate::Principal::unrestricted())
        .model()
        .deploy(&workflow, None)
        .await
        .unwrap();

    let (s, sig) = engine.signal(json!(null)).double();
    engine.channel().on_complete(move |e| {
        let s = s.clone();
        async move {
            s.send(e.outputs.to_value());
        }
    });
    engine.channel().on_error(move |e| async move {
        panic!("the loop must finish, not fail: {:?}", e.inputs);
    });
    engine
        .executor(&crate::Principal::unrestricted())
        .proc()
        .start(&workflow.id, Vars::new().with("input", 10))
        .await
        .unwrap();

    let outputs = timeout(Duration::from_secs(20), sig.recv())
        .await
        .expect("the loop never finished");
    assert_eq!(
        outputs,
        json!({ "result": 55 }),
        "the backward `next` must re-enter `cond` with the iteration's state"
    );
    engine.close().await;
}

/// A flow that loops without a guard that closes fails the run at
/// `max_node_run_times` instead of running forever (the legacy
/// `Process::create_task` bound).
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn an_unbounded_loop_fails_at_max_node_run_times() {
    let engine = Engine::builder()
        .max_node_run_times(5)
        .start()
        .await
        .unwrap();
    let workflow = Workflow::from_yml(
        r#"
id: runaway
ver: 0.1.0
vars:
  - name: hits
    value: 0
steps:
  - name: spinner
    id: spinner
    next: spinner
    uses: acts.transform.set
    params:
      hits: ${{ hits + 1 }}
"#,
    )
    .unwrap();
    engine
        .executor(&crate::Principal::unrestricted())
        .model()
        .deploy(&workflow, None)
        .await
        .unwrap();

    let (s, sig) = engine.signal(json!(null)).double();
    engine.channel().on_error(move |e| {
        let s = s.clone();
        async move {
            s.send(e.outputs.to_value());
        }
    });
    engine.channel().on_complete(move |_| async move {
        panic!("an unbounded loop must not complete");
    });
    engine
        .executor(&crate::Principal::unrestricted())
        .proc()
        .start(&workflow.id, Vars::new())
        .await
        .unwrap();

    let outputs = timeout(Duration::from_secs(20), sig.recv())
        .await
        .expect("the runaway loop never failed");
    assert_eq!(
        outputs.get("ecode"),
        Some(&json!("ELOOP")),
        "the bound reports itself: {outputs}"
    );
    engine.close().await;
}
