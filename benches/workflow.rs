use acts::{Engine, MessageState, Vars, Workflow};
use criterion::*;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;

/// Benchmark: parse workflow YAML — average time + QPS.
fn load(c: &mut Criterion) {
    let mut group = c.benchmark_group("load");
    group.throughput(Throughput::Elements(1));
    group.sample_size(10);

    group.bench_function("from_yml", |b| {
        let text = include_str!("./start.yml");
        b.iter_custom(move |iters| {
            let start = std::time::Instant::now();
            for _ in 0..iters {
                Workflow::from_yml(text).unwrap();
            }
            start.elapsed()
        })
    });

    group.finish();
}

/// Benchmark: deploy workflow — average time + QPS.
///
/// The engine is created per sample inside the measured future (outside the
/// timed region) and closed before the sample returns, so the whole run
/// happens on the one `rt` — no wrapper `block_on` needed. The previous form
/// registered the group inside `rt.block_on(..)`, mixing the engine runtime
/// with criterion's `FuturesExecutor` and dropping `rt` (and the engine's
/// writer task) without closing the engine.
fn deploy(c: &mut Criterion) {
    let mut group = c.benchmark_group("deploy");
    group.throughput(Throughput::Elements(1));
    group.sample_size(10);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let text = include_str!("./start.yml");
    let workflow = Workflow::from_yml(text).unwrap();

    group.bench_function("model", |b| {
        let workflow = workflow.clone();
        b.to_async(&rt).iter_custom(move |iters| {
            let workflow = workflow.clone();
            async move {
                let engine = Engine::builder()
                    .start()
                    .await
                    .expect("failed to start engine");
                let start = std::time::Instant::now();
                for _ in 0..iters {
                    engine
                        .executor(&acts::Principal::unrestricted())
                        .model()
                        .deploy(&workflow, None)
                        .await
                        .unwrap();
                }
                let elapsed = start.elapsed();
                engine.close().await;
                elapsed
            }
        })
    });
    group.finish();
}

/// Benchmark: start process — average time + QPS.
fn start(c: &mut Criterion) {
    let mut group = c.benchmark_group("start");
    group.throughput(Throughput::Elements(1));
    group.sample_size(10);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let text = include_str!("./start.yml");
    let workflow = Workflow::from_yml(text).unwrap();

    group.bench_function("proc", |b| {
        let workflow = workflow.clone();
        let workflow_id = workflow.id.clone();
        b.to_async(&rt).iter_custom(move |iters| {
            let workflow = workflow.clone();
            let workflow_id = workflow_id.clone();
            async move {
                let engine = Engine::builder()
                    .start()
                    .await
                    .expect("failed to start engine");
                engine
                    .executor(&acts::Principal::unrestricted())
                    .model()
                    .deploy(&workflow, None)
                    .await
                    .unwrap();

                let start = std::time::Instant::now();
                for _ in 0..iters {
                    engine
                        .executor(&acts::Principal::unrestricted())
                        .proc()
                        .start(&workflow_id, Vars::new())
                        .await
                        .unwrap();
                }
                let elapsed = start.elapsed();
                engine.close().await;
                elapsed
            }
        })
    });
    group.finish();
}

/// Completions timed as a single criterion iteration.
///
/// The batch size is fixed so criterion's auto-calibrated iteration count
/// cannot change how many `act().complete()` calls one iteration measures: the
/// untimed preparation (a process and an `acts.core.irq` message per
/// completion) is sized by this constant, never by the target warm-up or
/// measurement time.
const ACT_BATCH_SIZE: u64 = 8;

/// Hard cap for arming one batch. A missing or mismatched irq message must
/// fail the benchmark instead of blocking the wait forever.
const ACT_ARM_TIMEOUT: Duration = Duration::from_secs(10);

/// Start `count` processes and wait until every `acts.core.irq` task reports
/// `Created`, returning the `(pid, tid)` pairs to complete.
///
/// This is the untimed preparation for one batch: the workflow deployment and
/// the task arming all stay outside the measured region, and the wait is
/// bounded so a dropped message fails instead of hanging.
async fn arm_batch(engine: &Engine, workflow: &Workflow, count: u64) -> Vec<(String, String)> {
    let (fired, wait) = engine.signal(()).double();
    let tasks = Arc::new(Mutex::new(Vec::new()));
    let sink = tasks.clone();

    let chan = engine.channel();
    chan.on_message(move |e| {
        let sink = sink.clone();
        let fired = fired.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                let mut guard = sink.lock();
                guard.push((e.pid.clone(), e.tid.clone()));
                if guard.len() >= count as usize {
                    fired.close();
                }
            }
        }
    });

    for _ in 0..count {
        engine
            .executor(&acts::Principal::unrestricted())
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
    tasks
}

/// Benchmark: act().complete() — average time + QPS.
///
/// Each criterion iteration accounts for exactly `ACT_BATCH_SIZE` `complete()`
/// calls. The engine, the workflow deployment and every task armed for the
/// call are prepared outside the timer, and throughput is reported per
/// completed task.
fn act(c: &mut Criterion) {
    let mut group = c.benchmark_group("act");
    group.throughput(Throughput::Elements(ACT_BATCH_SIZE));
    group.sample_size(10);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let text = include_str!("./act.yml");
    let workflow = Workflow::from_yml(text).unwrap();

    group.bench_function("act", |b| {
        let workflow = workflow.clone();
        b.to_async(&rt).iter_custom(move |iters| {
            let workflow = workflow.clone();
            async move {
                let engine = Engine::builder()
                    .start()
                    .await
                    .expect("failed to start engine");
                engine
                    .executor(&acts::Principal::unrestricted())
                    .model()
                    .deploy(&workflow, None)
                    .await
                    .unwrap();

                let total = iters * ACT_BATCH_SIZE;
                let tasks = arm_batch(&engine, &workflow, total).await;

                let start = std::time::Instant::now();
                for (pid, tid) in &tasks {
                    engine
                        .executor(&acts::Principal::unrestricted())
                        .act()
                        .complete(pid, tid, Vars::new())
                        .await
                        .unwrap();
                }
                let elapsed = start.elapsed();
                engine.close().await;
                elapsed
            }
        })
    });
    group.finish();
}

criterion_group!(benches, load, deploy, start, act);
criterion_main!(benches);
