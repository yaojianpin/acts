//! Hotspot benchmarks for the engine runtime paths the store/workflow benches
//! do not cover: `${{ ... }}` expression evaluation, the pid-hashed scheduler
//! lanes under many concurrent processes, and emitter fan-out to many channel
//! handlers.

use acts::{Engine, MessageState, Vars, Workflow};
use criterion::*;
use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Hard cap for arming one batch — a missing event must fail the benchmark
/// instead of blocking the wait forever.
const ARM_TIMEOUT: Duration = Duration::from_secs(30);

/// Start `count` processes and wait until every `acts.core.irq` task reports
/// `Created`, returning the `(pid, tid)` pairs to complete and the
/// start→armed duration.
///
/// The duration covers the whole preparation — start, scheduler dispatch,
/// expression filling, message emission, handler dispatch — which is exactly
/// the span `expr_eval` measures; the other groups only use the pairs.
async fn arm_batch(
    engine: &Engine,
    workflow: &Workflow,
    count: u64,
) -> (Vec<(String, String)>, Duration) {
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

    let start = Instant::now();
    for _ in 0..count {
        engine
            .executor()
            .proc()
            .start(&workflow.id, Vars::new())
            .await
            .unwrap();
    }
    tokio::time::timeout(ARM_TIMEOUT, wait.recv())
        .await
        .expect("timed out arming acts.core.irq tasks");
    let elapsed = start.elapsed();
    chan.close();

    let mut tasks = std::mem::take(&mut *tasks.lock());
    tasks.truncate(count as usize);
    (tasks, elapsed)
}

/// A workflow whose `acts.core.irq` params carry `exprs` `${{ ... }}`
/// expressions. Starting a process evaluates every one of them while filling
/// the act params (`Task::params` → `fill_params` → `Environment::eval`).
fn expr_workflow(exprs: usize) -> Workflow {
    let mut params = String::from("      key: act1\n");
    for i in 0..exprs {
        params.push_str(&format!(
            "      v{}: '${{{{ Math.sqrt({}) * 1000 }}}}'\n",
            i, i
        ));
    }

    let text = format!(
        "id: expr_bench\nver: 0.1.0\nsteps:\n  - id: step1\n    uses: acts.core.irq\n    params:\n{params}"
    );
    Workflow::from_yml(&text).unwrap()
}

/// Benchmark: `${{ ... }}` expression evaluation, timed end to end.
///
/// The expression environment is crate-private, so a process is started with
/// `exprs` expressions in its irq params and the start→armed span is timed:
/// the scheduler and emitter costs are constant across the variants, so the
/// slope is the per-expression cost (a fresh QuickJS realm, module init and
/// JS→JSON conversion).
fn expr_eval(c: &mut Criterion) {
    let mut group = c.benchmark_group("expr_eval");
    group.throughput(Throughput::Elements(1));
    group.sample_size(10);

    let rt = tokio::runtime::Runtime::new().unwrap();

    for &exprs in &[1usize, 16, 64] {
        let workflow = expr_workflow(exprs);
        group.bench_function(BenchmarkId::new("irq_params", exprs.to_string()), |b| {
            let workflow = workflow.clone();
            b.to_async(&rt).iter_custom(move |iters| {
                let workflow = workflow.clone();
                async move {
                    let engine = Engine::builder()
                        .start()
                        .await
                        .expect("failed to start engine");
                    engine
                        .executor()
                        .model()
                        .deploy(&workflow, None)
                        .await
                        .unwrap();

                    let mut total = Duration::ZERO;
                    for _ in 0..iters {
                        let (mut tasks, elapsed) = arm_batch(&engine, &workflow, 1).await;
                        total += elapsed;
                        // complete the process so it is evicted between
                        // iterations instead of accumulating
                        if let Some((pid, tid)) = tasks.pop() {
                            engine
                                .executor()
                                .act()
                                .complete(&pid, &tid, Vars::new())
                                .await
                                .unwrap();
                        }
                    }
                    engine.close().await;
                    total
                }
            })
        });
    }
    group.finish();
}

/// Workflow for the scheduler group: four no-op steps, so every process runs
/// its root task plus one task per step through the pid-hashed lanes.
const SCHED_WORKFLOW: &str = r#"
id: sched_bench
ver: 0.1.0
steps:
  - id: s1
  - id: s2
  - id: s3
  - id: s4
"#;

/// Processes started per criterion iteration, so the auto-calibrated iteration
/// count cannot change how many pids contend for the lanes at once.
const PID_BATCH: u64 = 32;

/// Benchmark: many concurrent processes against the fixed pid-hashed lanes.
///
/// Each iteration starts `PID_BATCH` processes and waits for all of them to
/// complete; `scheduler_workers` sets the lane count so the group shows the
/// overlap the lanes buy (1 = serialized, 4 = four pids in flight).
fn scheduler_multi_pid(c: &mut Criterion) {
    let mut group = c.benchmark_group("scheduler_multi_pid");
    group.throughput(Throughput::Elements(PID_BATCH));
    group.sample_size(10);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let workflow = Workflow::from_yml(SCHED_WORKFLOW).unwrap();

    for &workers in &[1usize, 4] {
        group.bench_function(BenchmarkId::new("workers", workers.to_string()), |b| {
            let workflow = workflow.clone();
            b.to_async(&rt).iter_custom(move |iters| {
                let workflow = workflow.clone();
                async move {
                    let engine = Engine::builder()
                        .scheduler_workers(workers)
                        .start()
                        .await
                        .expect("failed to start engine");
                    engine
                        .executor()
                        .model()
                        .deploy(&workflow, None)
                        .await
                        .unwrap();

                    let total = iters * PID_BATCH;
                    let (fired, wait) = engine.signal(()).double();
                    let done = Arc::new(AtomicU64::new(0));
                    let chan = engine.channel();
                    let counter = done.clone();
                    let fire = fired.clone();
                    chan.on_complete(move |e| {
                        let counter = counter.clone();
                        let fire = fire.clone();
                        async move {
                            if e.is_type("workflow")
                                && e.is_state(MessageState::Completed)
                                && counter.fetch_add(1, Ordering::AcqRel) + 1 >= total
                            {
                                fire.close();
                            }
                        }
                    });

                    let start = Instant::now();
                    for _ in 0..total {
                        engine
                            .executor()
                            .proc()
                            .start(&workflow.id, Vars::new())
                            .await
                            .unwrap();
                    }
                    tokio::time::timeout(ARM_TIMEOUT, wait.recv())
                        .await
                        .expect("timed out waiting for process completions");
                    let elapsed = start.elapsed();
                    chan.close();
                    engine.close().await;
                    elapsed
                }
            })
        });
    }
    group.finish();
}

/// Handlers one emitted message fans out to (`engine.channel()` per handler,
/// as a transport client would register).
const FANOUT_HANDLERS: [u64; 3] = [1, 16, 64];

/// Completions timed per criterion iteration — a fixed batch, so the
/// calibrated iteration count cannot change the fan-out width.
const FANOUT_BATCH: u64 = 8;

/// Benchmark: emitter fan-out — one workflow-completion event delivered to
/// every registered channel handler.
///
/// Each iteration arms `FANOUT_BATCH` irq tasks outside the timer and then
/// registers `handlers` channels, so only the timed `complete()` pays for
/// them: a completion emits one workflow-complete event that every handler
/// receives (glob match + isolated spawn), and the loop waits for all
/// `handlers` deliveries before accounting the next completion.
fn emitter_fanout(c: &mut Criterion) {
    let mut group = c.benchmark_group("emitter_fanout");
    group.throughput(Throughput::Elements(FANOUT_BATCH));
    group.sample_size(10);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let workflow = Workflow::from_yml(include_str!("./act.yml")).unwrap();

    for &handlers in &FANOUT_HANDLERS {
        group.bench_function(BenchmarkId::new("channels", handlers.to_string()), |b| {
            let workflow = workflow.clone();
            b.to_async(&rt).iter_custom(move |iters| {
                let workflow = workflow.clone();
                async move {
                    let engine = Engine::builder()
                        .start()
                        .await
                        .expect("failed to start engine");
                    engine
                        .executor()
                        .model()
                        .deploy(&workflow, None)
                        .await
                        .unwrap();

                    // arm the completions outside the timed region, then
                    // register the fan-out channels so only `complete()` pays
                    let (tasks, _) = arm_batch(&engine, &workflow, iters * FANOUT_BATCH).await;

                    let (tx, mut rx) = mpsc::unbounded_channel::<()>();
                    let mut channels = Vec::with_capacity(handlers as usize);
                    for _ in 0..handlers {
                        let chan = engine.channel();
                        let tx = tx.clone();
                        chan.on_complete(move |e| {
                            let tx = tx.clone();
                            async move {
                                if e.is_type("workflow") && e.is_state(MessageState::Completed) {
                                    let _ = tx.send(());
                                }
                            }
                        });
                        channels.push(chan);
                    }
                    drop(tx);

                    let mut total = Duration::ZERO;
                    for (pid, tid) in &tasks {
                        let start = Instant::now();
                        engine
                            .executor()
                            .act()
                            .complete(pid, tid, Vars::new())
                            .await
                            .unwrap();
                        for _ in 0..handlers {
                            rx.recv().await.expect("fan-out handler dropped");
                        }
                        total += start.elapsed();
                    }

                    for chan in &channels {
                        chan.close();
                    }
                    engine.close().await;
                    total
                }
            })
        });
    }
    group.finish();
}

criterion_group!(benches, expr_eval, scheduler_multi_pid, emitter_fanout,);
criterion_main!(benches);
