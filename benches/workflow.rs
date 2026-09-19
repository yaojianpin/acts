use acts::{Engine, MessageState, Principal, Vars, Workflow};
use criterion::*;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Benchmark: parse workflow YAML.
///
/// Measures one complete `Workflow::from_yml` operation.
fn load(c: &mut Criterion) {
    let mut group = c.benchmark_group("load");

    group.throughput(Throughput::Elements(1));

    group.sample_size(10);

    group.bench_function("from_yml", |b| {
        let text = include_str!("./start.yml");

        b.iter_custom(move |iters| {
            let start = Instant::now();

            for _ in 0..iters {
                Workflow::from_yml(text).unwrap();
            }

            start.elapsed()
        });
    });

    group.finish();
}

/// Benchmark: deploy one parsed workflow repeatedly.
///
/// Engine construction and shutdown are outside the measured region. The
/// measured duration contains only repeated model deployment.
fn deploy(c: &mut Criterion) {
    let mut group = c.benchmark_group("deploy");

    group.throughput(Throughput::Elements(1));

    group.sample_size(10);

    let runtime = tokio::runtime::Runtime::new().unwrap();

    let text = include_str!("./start.yml");

    let workflow = Workflow::from_yml(text).unwrap();

    group.bench_function("model", |b| {
        let workflow = workflow.clone();

        b.to_async(&runtime).iter_custom(move |iters| {
            let workflow = workflow.clone();

            async move {
                let engine = Engine::builder()
                    .start()
                    .await
                    .expect("failed to start engine");

                let principal = Principal::unrestricted();

                let executor = engine.executor(&principal);

                let start = Instant::now();

                for _ in 0..iters {
                    executor.model().deploy(&workflow, None).await.unwrap();
                }

                let elapsed = start.elapsed();

                engine.close().await;

                elapsed
            }
        });
    });

    group.finish();
}

/// Benchmark: submit process starts repeatedly.
///
/// Engine startup and workflow deployment are outside the measured region.
/// The measured duration contains calls to `proc().start()`.
///
/// This benchmark measures start admission/API cost. It does not wait for each
/// workflow to reach its terminal state before submitting the next start.
fn start(c: &mut Criterion) {
    let mut group = c.benchmark_group("start");

    group.throughput(Throughput::Elements(1));

    group.sample_size(10);

    let runtime = tokio::runtime::Runtime::new().unwrap();

    let text = include_str!("./start.yml");

    let workflow = Workflow::from_yml(text).unwrap();

    group.bench_function("proc", |b| {
        let workflow = workflow.clone();

        let workflow_id = workflow.id.clone();

        b.to_async(&runtime).iter_custom(move |iters| {
            let workflow = workflow.clone();

            let workflow_id = workflow_id.clone();

            async move {
                let engine = Engine::builder()
                    .start()
                    .await
                    .expect("failed to start engine");

                let principal = Principal::unrestricted();

                let executor = engine.executor(&principal);

                executor.model().deploy(&workflow, None).await.unwrap();

                let start = Instant::now();

                for _ in 0..iters {
                    executor
                        .proc()
                        .start(&workflow_id, Vars::new())
                        .await
                        .unwrap();
                }

                let elapsed = start.elapsed();

                engine.close().await;

                elapsed
            }
        });
    });

    group.finish();
}

/// Number of completions measured per Criterion iteration.
///
/// Criterion controls how many independent benchmark iterations run.
/// `ACT_BATCH_SIZE` controls how many completion operations belong to one
/// iteration. Keeping the batch fixed prevents Criterion calibration from
/// expanding the resident task set.
const ACT_BATCH_SIZE: u64 = 8;

/// Maximum time allowed to prepare one fixed completion batch.
///
/// A missing or mismatched IRQ-created event must fail the benchmark instead
/// of leaving the benchmark blocked indefinitely.
const ACT_ARM_TIMEOUT: Duration = Duration::from_secs(10);

/// Start exactly `count` processes and wait until each `acts.core.irq` task
/// reports the `Created` state.
///
/// Task preparation is intentionally outside the measured completion region.
/// The returned `(pid, tid)` pairs identify the tasks that the benchmark will
/// complete.
///
/// The channel is closed before returning, and the returned collection is
/// truncated to `count` in case multiple callbacks crossed the completion
/// threshold concurrently.
async fn arm_batch(
    engine: &Engine,
    executor: &acts::Executor,
    workflow: &Workflow,
    count: u64,
) -> Vec<(String, String)> {
    assert!(count > 0, "the benchmark arm batch must not be empty",);

    let (fired, wait) = engine.signal(()).double();

    let tasks = Arc::new(Mutex::new(Vec::with_capacity(count as usize)));

    let sink = tasks.clone();

    let chan = engine.channel();

    chan.on_message(move |event| {
        let sink = sink.clone();

        let fired = fired.clone();

        async move {
            if event.is_params_key("act1") && event.is_state(MessageState::Created) {
                let mut guard = sink.lock();

                guard.push((event.pid.clone(), event.tid.clone()));

                if guard.len() >= count as usize {
                    fired.close();
                }
            }
        }
    });

    for _ in 0..count {
        executor
            .proc()
            .start(&workflow.id, Vars::new())
            .await
            .unwrap();
    }

    tokio::time::timeout(ACT_ARM_TIMEOUT, wait.recv())
        .await
        .expect("timed out arming acts.core.irq tasks");

    chan.close();

    let mut tasks = std::mem::take(&mut *tasks.lock());

    tasks.truncate(count as usize);

    assert_eq!(
        tasks.len(),
        count as usize,
        "armed task count does not match requested batch size",
    );

    tasks
}

/// Benchmark: complete a fixed batch of IRQ tasks.
///
/// One Criterion iteration always measures exactly `ACT_BATCH_SIZE` completion
/// operations.
///
/// Engine creation, workflow deployment, and task arming are outside the
/// measured interval. Only calls to `act().complete()` are accumulated.
///
/// This fixed-batch structure is required because Criterion may increase
/// `iters` significantly during calibration. Multiplying the preparation batch
/// by `iters` can exceed the resident-process limit and park tasks before any
/// completion is allowed to release capacity.
fn act(c: &mut Criterion) {
    let mut group = c.benchmark_group("act");

    group.throughput(Throughput::Elements(ACT_BATCH_SIZE));

    group.sample_size(10);

    let runtime = tokio::runtime::Runtime::new().unwrap();

    let text = include_str!("./act.yml");

    let workflow = Workflow::from_yml(text).unwrap();

    group.bench_function("act", |b| {
        let workflow = workflow.clone();

        b.to_async(&runtime).iter_custom(move |iters| {
            let workflow = workflow.clone();

            async move {
                let engine = Engine::builder()
                    .start()
                    .await
                    .expect("failed to start engine");

                let principal = Principal::unrestricted();

                let executor = engine.executor(&principal);

                executor.model().deploy(&workflow, None).await.unwrap();

                let mut measured = Duration::ZERO;

                for _ in 0..iters {
                    // Keep the resident preparation batch fixed.
                    //
                    // Criterion's `iters` controls how many batches
                    // are measured. It must not increase the number
                    // of tasks prepared simultaneously.
                    let tasks = arm_batch(&engine, &executor, &workflow, ACT_BATCH_SIZE).await;

                    let start = Instant::now();

                    for (pid, tid) in &tasks {
                        executor
                            .act()
                            .complete(pid, tid, Vars::new())
                            .await
                            .unwrap();
                    }

                    measured += start.elapsed();
                }

                engine.close().await;

                measured
            }
        });
    });

    group.finish();
}

criterion_group!(benches, load, deploy, start, act,);

criterion_main!(benches);
