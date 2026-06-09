use acts::{Engine, MessageState, Vars, Workflow};
use criterion::async_executor::FuturesExecutor;
use criterion::*;
use std::sync::{Arc, Mutex};

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
fn deploy(c: &mut Criterion) {
    let mut group = c.benchmark_group("deploy");
    group.throughput(Throughput::Elements(1));
    group.sample_size(10);

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async move {
        let engine = Engine::new().start().expect("failed to start engine");
        let text = include_str!("./start.yml");
        let workflow = Workflow::from_yml(text).unwrap();

        group.bench_function("model", |b| {
            let engine = engine.clone();
            let workflow = workflow.clone();
            b.to_async(FuturesExecutor).iter_custom(move |iters| {
                let engine = engine.clone();
                let workflow = workflow.clone();
                async move {
                    let start = std::time::Instant::now();
                    for _ in 0..iters {
                        engine.executor().model().deploy(&workflow).unwrap();
                    }
                    start.elapsed()
                }
            })
        });
        group.finish();
    });
}

/// Benchmark: start process — average time + QPS.
fn start(c: &mut Criterion) {
    let mut group = c.benchmark_group("start");
    group.throughput(Throughput::Elements(1));
    group.sample_size(10);

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async move {
        let engine = Engine::new().start().expect("failed to start engine");
        let text = include_str!("./start.yml");
        let workflow = Workflow::from_yml(text).unwrap();
        engine.executor().model().deploy(&workflow).unwrap();

        group.bench_function("proc", |b| {
            let engine = engine.clone();
            let workflow_id = workflow.id.clone();
            b.to_async(FuturesExecutor).iter_custom(move |iters| {
                let engine = engine.clone();
                let workflow_id = workflow_id.clone();
                async move {
                    let start = std::time::Instant::now();
                    for _ in 0..iters {
                        engine
                            .executor()
                            .proc()
                            .start(&workflow_id, Vars::new())
                            .unwrap();
                    }
                    start.elapsed()
                }
            })
        });
        group.finish();
    });
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
                let engine = Engine::new().start().expect("failed to start engine");
                engine.executor().model().deploy(&workflow).unwrap();

                let (s, sig) = engine.signal(()).double();
                let tasks = Arc::new(Mutex::new(Vec::new()));
                let tasks2 = tasks.clone();

                let chan = engine.channel();
                chan.on_message(move |e| {
                    if e.is_nid("act1") && e.is_state(MessageState::Created) {
                        let mut t = tasks2.lock().unwrap();
                        t.push((e.pid.clone(), e.tid.clone()));
                        if t.len() >= iters as usize {
                            s.close();
                        }
                    }
                });

                // Pre-create tasks
                for _ in 0..iters {
                    engine
                        .executor()
                        .proc()
                        .start(&workflow.id, Vars::new())
                        .unwrap();
                }
                sig.recv().await;

                let tasks = tasks.lock().unwrap();

                // Only time the act().complete() calls
                let bench_start = std::time::Instant::now();
                for (pid, tid) in tasks.iter() {
                    engine
                        .executor()
                        .act()
                        .complete(pid, tid, Vars::new())
                        .unwrap();
                }
                bench_start.elapsed()
            }
        })
    });
    group.finish();
}

criterion_group!(benches, load, deploy, start, act);
criterion_main!(benches);
