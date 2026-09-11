use crate::{
    ActPackage, ActPackageCatalog, ActPackageDefinition, ActRunAs, Config, Engine, Vars, Workflow,
    config::ConfigData, utils::longid,
};
use serde_json::json;
use serial_test::serial;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::time::timeout;

static ACTIVE_JOBS: AtomicUsize = AtomicUsize::new(0);
static MAX_ACTIVE_JOBS: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone)]
struct SlowPackage;

#[async_trait::async_trait]
impl ActPackage for SlowPackage {
    fn definition() -> ActPackageDefinition {
        ActPackageDefinition {
            id: "test.scheduler.slow",
            name: "Slow",
            desc: "sleep to expose scheduler concurrency",
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
        _ctx: &crate::Context,
        _params: &serde_json::Value,
    ) -> crate::Result<Option<Vars>> {
        let active = ACTIVE_JOBS.fetch_add(1, Ordering::SeqCst) + 1;
        MAX_ACTIVE_JOBS.fetch_max(active, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(100)).await;
        ACTIVE_JOBS.fetch_sub(1, Ordering::SeqCst);
        Ok(None)
    }
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn scheduler_lanes_overlap_independent_processes() {
    const PROCESS_COUNT: usize = 16;
    let config = Config {
        data: ConfigData {
            scheduler_workers: Some(4),
            ..Default::default()
        },
        table: Default::default(),
    };
    let engine = Engine::builder()
        .set_config(&config)
        .add_package::<SlowPackage>()
        .start()
        .await
        .unwrap();

    let completed = Arc::new(AtomicUsize::new(0));
    let completion_count = completed.clone();
    engine.channel().on_complete(move |_| {
        completion_count.fetch_add(1, Ordering::SeqCst);
        async move {}
    });

    let workflow = Workflow::new()
        .with_id("scheduler-lanes")
        .with_step(|step| {
            step.with_id("slow-step")
                .with_uses("test.scheduler.slow", Vars::new())
        });
    for _ in 0..PROCESS_COUNT {
        engine
            .runtime()
            .start(&workflow, Vars::new().with("pid", longid()))
            .await
            .unwrap();
    }

    let started = Instant::now();
    timeout(Duration::from_secs(5), async {
        while completed.load(Ordering::SeqCst) < PROCESS_COUNT {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("all independent processes must finish");

    assert!(
        MAX_ACTIVE_JOBS.load(Ordering::SeqCst) > 1,
        "independent pids must execute on multiple lanes"
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "16 × 100ms jobs must overlap on four lanes: elapsed={:?}, max_active={}",
        started.elapsed(),
        MAX_ACTIVE_JOBS.load(Ordering::SeqCst)
    );
    engine.close().await;
}
