use crate::{
    ActPackage, ActPackageCatalog, ActPackageDefinition, ActRunAs, Config, Engine, Vars, Workflow,
    config::ConfigData, data, utils::longid,
};
use serde_json::json;
use serial_test::serial;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{sync::Notify, time::timeout};

/// Lanes the overlap test configures the engine with.
const LANES: usize = 4;

/// Longest a gated job waits for its partners. Bounded so the only thing that
/// can consume it is the straggler that arrives with no partner left to pair
/// with — a real concurrency failure is then reported by the assertions below,
/// not by an unrelated test timeout.
const GATE_WAIT: Duration = Duration::from_millis(250);

static ACTIVE_JOBS: AtomicUsize = AtomicUsize::new(0);
static MAX_ACTIVE_JOBS: AtomicUsize = AtomicUsize::new(0);
/// Jobs that must be executing at once before any of them returns. Armed by the
/// concurrency test so "do the lanes overlap?" is decided by the scheduler, not
/// by how loaded the machine is (aggregate throughput is the
/// `scheduler_multi_pid` bench's job).
static REQUIRED_IN_FLIGHT: AtomicUsize = AtomicUsize::new(1);
/// Latched once that concurrency has been observed. Overlap only has to happen
/// once to be established, so later jobs stop waiting — otherwise every job of
/// the run would pay the wait and the test would spend its time budget on
/// stragglers instead of on the assertion.
static GATE_MET: AtomicBool = AtomicBool::new(false);
static IN_FLIGHT_GATE: Notify = Notify::const_new();

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
        let peak = MAX_ACTIVE_JOBS
            .fetch_max(active, Ordering::SeqCst)
            .max(active);
        if peak >= REQUIRED_IN_FLIGHT.load(Ordering::SeqCst) {
            // the concurrency this run is looking for just happened (or already
            // had): latch it and release every waiter
            GATE_MET.store(true, Ordering::SeqCst);
            IN_FLIGHT_GATE.notify_waiters();
        }
        if REQUIRED_IN_FLIGHT.load(Ordering::SeqCst) > 1 && !GATE_MET.load(Ordering::SeqCst) {
            // Hold this job until `required` of them run at once. Every arrival
            // notifies before it waits and the condition is re-checked after
            // each wake, so the job that completes the count cannot be missed.
            IN_FLIGHT_GATE.notify_waiters();
            let _ = timeout(GATE_WAIT, async {
                while !GATE_MET.load(Ordering::SeqCst) {
                    IN_FLIGHT_GATE.notified().await;
                }
            })
            .await;
        }
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
            scheduler_workers: Some(LANES),
            ..Default::default()
        },
        table: Default::default(),
    };
    // Every job holds until a second one is running, so overlap is a fact the
    // scheduler has to produce rather than a wall-clock margin a loaded machine
    // can eat into (aggregate throughput is the `scheduler_multi_pid` bench).
    MAX_ACTIVE_JOBS.store(0, Ordering::SeqCst);
    GATE_MET.store(false, Ordering::SeqCst);
    REQUIRED_IN_FLIGHT.store(2, Ordering::SeqCst);

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
    // generous: this test asserts on the concurrency it observed, not on how
    // long the run took, so the deadline only guards against a hang
    timeout(Duration::from_secs(30), async {
        while completed.load(Ordering::SeqCst) < PROCESS_COUNT {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("all independent processes must finish");

    let max_active = MAX_ACTIVE_JOBS.load(Ordering::SeqCst);
    assert!(
        max_active > 1,
        "independent pids must execute on multiple lanes: max_active={max_active}"
    );
    assert!(
        max_active <= LANES,
        "the lanes are the in-flight cap: max_active={max_active}, lanes={LANES}"
    );
    REQUIRED_IN_FLIGHT.store(1, Ordering::SeqCst);
    engine.close().await;
}

/// A lane is a real bound, not a promise: one worker with a one-slot lane
/// cannot buffer every process's work in memory while it executes a slow job.
/// The starts past the bound are refused by the lane and written to the durable
/// outbox, which the overflow consumer replays — the work is not lost, the
/// resident backlog never exceeds the configured cap, and the reported depth is
/// that real backlog.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_saturated_lane_overflows_to_the_durable_outbox() {
    const PROCESSES: usize = 6;
    let config = Config {
        data: ConfigData {
            scheduler_workers: Some(1),
            scheduler_queue_cap: Some(1),
            ..Default::default()
        },
        table: Default::default(),
    };
    // this test measures the backlog, not the overlap: no job waits for another
    REQUIRED_IN_FLIGHT.store(1, Ordering::SeqCst);
    let engine = Engine::builder()
        .set_config(&config)
        .add_package::<SlowPackage>()
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();
    assert_eq!(rt.scheduler_lane_capacity(), 1);

    let completed = Arc::new(AtomicUsize::new(0));
    let completion_count = completed.clone();
    engine.channel().on_complete(move |_| {
        completion_count.fetch_add(1, Ordering::SeqCst);
        async move {}
    });

    // every start needs the single lane for longer than it takes to issue the
    // next one, so the starts past the lane's one slot can only be durable
    let workflow = Workflow::new().with_id("lane-overflow").with_step(|step| {
        step.with_id("slow-step")
            .with_uses("test.scheduler.slow", Vars::new())
    });
    let mut procs = Vec::with_capacity(PROCESSES);
    for _ in 0..PROCESSES {
        procs.push(
            rt.start(&workflow, Vars::new().with("pid", longid()))
                .await
                .unwrap(),
        );
    }

    // Watch the overload while it happens: a start refused by the lane exists
    // only as a durable outbox row until the overflow consumer replays it, and
    // the resident backlog stays within the cap throughout.
    let mut spilled = 0usize;
    let mut peak_depth = 0usize;
    let deadline = Instant::now() + Duration::from_secs(30);
    while completed.load(Ordering::SeqCst) < PROCESSES && Instant::now() < deadline {
        peak_depth = peak_depth.max(rt.scheduler_queue_depth());
        let ops = rt.store().load_pending_ops().await.unwrap();
        spilled = spilled.max(
            ops.iter()
                .filter(|op| op.r#type == data::OpType::Exec.as_ref())
                .count(),
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert!(
        spilled > 0,
        "starts past the lane bound must overflow to the durable outbox"
    );
    assert!(
        peak_depth <= rt.scheduler_queue_capacity(),
        "the resident backlog must stay within the cap: depth={peak_depth}"
    );
    assert_eq!(
        completed.load(Ordering::SeqCst),
        PROCESSES,
        "every spilled start must be replayed and finish"
    );
    for proc in procs {
        assert!(
            proc.state().is_biz_success(),
            "process {} ended in {:?}",
            proc.id(),
            proc.state()
        );
    }
    engine.close().await;
}
