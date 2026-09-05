use acts::{Engine, MessageState, Vars, Workflow};
use criterion::*;
use parking_lot::Mutex;
use std::sync::Arc;

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
                let engine = Engine::new().start().await.expect("failed to start engine");
                let start = std::time::Instant::now();
                for _ in 0..iters {
                    engine
                        .executor()
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
                let engine = Engine::new().start().await.expect("failed to start engine");
                engine
                    .executor()
                    .model()
                    .deploy(&workflow, None)
                    .await
                    .unwrap();

                let start = std::time::Instant::now();
                for _ in 0..iters {
                    engine
                        .executor()
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

/// Benchmark: act().complete() — average time + QPS.
///
/// Pre-creates tasks via proc.start(), then only times the complete() calls.
fn act(c: &mut Criterion) {
    let mut group = c.benchmark_group("act");
    group.throughput(Throughput::Elements(1));
    group.sample_size(10);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let text = include_str!("./act.yml");
    let workflow = Workflow::from_yml(text).unwrap();

    group.bench_function("act", |b| {
        let workflow = workflow.clone();
        b.to_async(&rt).iter_custom(move |iters| {
            let workflow = workflow.clone();
            async move {
                let engine = Engine::new().start().await.expect("failed to start engine");
                engine
                    .executor()
                    .model()
                    .deploy(&workflow, None)
                    .await
                    .unwrap();

                let (s, sig) = engine.signal(()).double();
                let tasks = Arc::new(Mutex::new(Vec::new()));
                let tasks2 = tasks.clone();

                let chan = engine.channel();
                chan.on_message(move |e| {
                    let tasks2 = tasks2.clone();
                    let s = s.clone();
                    async move {
                        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                            let mut t = tasks2.lock();
                            t.push((e.pid.clone(), e.tid.clone()));
                            if t.len() >= iters as usize {
                                s.close();
                            }
                        }
                    }
                });

                // Pre-create tasks
                for _ in 0..iters {
                    engine
                        .executor()
                        .proc()
                        .start(&workflow.id, Vars::new())
                        .await
                        .unwrap();
                }
                sig.recv().await;

                let tasks = {
                    let guard = tasks.lock();
                    guard.clone()
                };

                chan.close();

                // Only time the act().complete() calls
                let bench_start = std::time::Instant::now();
                for (pid, tid) in tasks.iter() {
                    engine
                        .executor()
                        .act()
                        .complete(pid, tid, Vars::new())
                        .await
                        .unwrap();
                }
                let elapsed = bench_start.elapsed();
                engine.close().await;
                elapsed
            }
        })
    });
    group.finish();
}

criterion_group!(benches, load, deploy, start, act);
criterion_main!(benches);
