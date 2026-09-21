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

/// Hard cap for arming one batch. A missing event must fail the benchmark
/// instead of blocking the wait forever.
const ARM_TIMEOUT: Duration = Duration::from_secs(30);

/// Start `count` processes and wait until every `acts.core.irq` task reports
/// `Created`, returning the `(pid, tid)` pairs to complete and the
/// start-to-armed duration.
///
/// The duration covers the whole preparation: process start, scheduler
/// dispatch, expression filling, message emission, and handler dispatch.
/// This is exactly the span `expr_eval` measures. Other benchmark groups use
/// only the returned task pairs and keep this duration outside their timer.
async fn arm_batch(
    engine: &Engine,
    workflow: &Workflow,
    count: u64,
) -> (Vec<(String, String)>, Duration) {
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

    let start = Instant::now();

    for _ in 0..count {
        engine
            .executor(&acts::Principal::unrestricted())
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
/// expressions.
fn expr_workflow(exprs: usize) -> Workflow {
    let mut params = String::from("      key: act1\n");

    for i in 0..exprs {
        params.push_str(&format!("      v{}: '${{{{ ({} * 1000) }}}}'\n", i, i));
    }

    let text = format!(
        "id: expr_bench\n\
         ver: 0.1.0\n\
         steps:\n\
         \x20 - id: step1\n\
         \x20   uses: acts.core.irq\n\
         \x20   params:\n\
         {params}"
    );

    Workflow::from_yml(&text).unwrap()
}

/// Benchmark: `${{ ... }}` expression evaluation, timed end to end.
///
/// The expression environment is crate-private, so a process is started with
/// `exprs` expressions in its IRQ params. The benchmark measures the
/// start-to-armed span.
///
/// Scheduler and emitter costs remain approximately constant across the
/// variants. The slope therefore represents the incremental cost per
/// expression, including CEL compilation, variable injection, and evaluation
/// to a JSON value.
fn expr_eval(c: &mut Criterion) {
    let mut group = c.benchmark_group("expr_eval");

    group.throughput(Throughput::Elements(1));

    group.sample_size(10);

    let runtime = tokio::runtime::Runtime::new().unwrap();

    for &exprs in &[1usize, 16, 64] {
        let workflow = expr_workflow(exprs);

        group.bench_function(BenchmarkId::new("irq_params", exprs.to_string()), |b| {
            let workflow = workflow.clone();

            b.to_async(&runtime).iter_custom(move |iters| {
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

                    let mut total = Duration::ZERO;

                    for _ in 0..iters {
                        let (mut tasks, elapsed) = arm_batch(&engine, &workflow, 1).await;

                        total += elapsed;

                        // Complete the process outside the measured
                        // start-to-armed duration so completed
                        // processes do not accumulate across
                        // iterations.
                        if let Some((pid, tid)) = tasks.pop() {
                            engine
                                .executor(&acts::Principal::unrestricted())
                                .act()
                                .complete(&pid, &tid, Vars::new())
                                .await
                                .unwrap();
                        }
                    }

                    engine.close().await;

                    total
                }
            });
        });
    }

    group.finish();
}

/// Workflow for the scheduler benchmark.
///
/// Four no-op steps cause every process to execute its root task and one task
/// for each step through the PID-hashed scheduler lanes.
const SCHED_WORKFLOW: &str = r#"
id: sched_bench
ver: 0.1.0
steps:
  - id: s1
  - id: s2
  - id: s3
  - id: s4
"#;

/// Processes started per Criterion iteration.
///
/// This value is deliberately fixed. Criterion's auto-calibrated `iters`
/// controls how many independent batches are measured. It must not change the
/// number of PIDs contending for scheduler lanes inside one batch.
const PID_BATCH: u64 = 32;

/// Benchmark: fixed bursts of concurrent processes against the PID-hashed
/// scheduler lanes.
///
/// Each Criterion iteration starts exactly `PID_BATCH` processes and waits for
/// all processes in that batch to complete. `scheduler_workers` controls the
/// lane count.
///
/// The measured duration covers process submission through workflow completion
/// for the entire fixed batch.
fn scheduler_multi_pid(c: &mut Criterion) {
    let mut group = c.benchmark_group("scheduler_multi_pid");

    group.throughput(Throughput::Elements(PID_BATCH));

    group.sample_size(10);

    let runtime = tokio::runtime::Runtime::new().unwrap();

    let workflow = Workflow::from_yml(SCHED_WORKFLOW).unwrap();

    for &workers in &[1usize, 4] {
        group.bench_function(BenchmarkId::new("workers", workers.to_string()), |b| {
            let workflow = workflow.clone();

            b.to_async(&runtime).iter_custom(move |iters| {
                let workflow = workflow.clone();

                async move {
                    let engine = Engine::builder()
                        .scheduler_workers(workers)
                        .start()
                        .await
                        .expect("failed to start engine");

                    engine
                        .executor(&acts::Principal::unrestricted())
                        .model()
                        .deploy(&workflow, None)
                        .await
                        .unwrap();

                    let mut elapsed = Duration::ZERO;

                    // Keep each scheduler burst fixed. Criterion's
                    // `iters` controls repetition count, not the
                    // number of simultaneously active processes.
                    for _ in 0..iters {
                        let (fired, wait) = engine.signal(()).double();

                        let completed = Arc::new(AtomicU64::new(0));

                        let chan = engine.channel();

                        let counter = completed.clone();

                        let fire = fired.clone();

                        chan.on_complete(move |event| {
                            let counter = counter.clone();

                            let fire = fire.clone();

                            async move {
                                if event.is_type("workflow")
                                    && event.is_state(MessageState::Completed)
                                    && counter.fetch_add(1, Ordering::AcqRel) + 1 >= PID_BATCH
                                {
                                    fire.close();
                                }
                            }
                        });

                        let start = Instant::now();

                        for _ in 0..PID_BATCH {
                            engine
                                .executor(&acts::Principal::unrestricted())
                                .proc()
                                .start(&workflow.id, Vars::new())
                                .await
                                .unwrap();
                        }

                        tokio::time::timeout(ARM_TIMEOUT, wait.recv())
                            .await
                            .expect("timed out waiting for process completions");

                        elapsed += start.elapsed();

                        // The iteration does not return until every
                        // process in this batch has completed, so no
                        // completion belonging to this batch is left
                        // for the next iteration.
                        chan.close();
                    }

                    engine.close().await;

                    elapsed
                }
            });
        });
    }

    group.finish();
}

/// Handler counts used by the emitter fan-out benchmark.
///
/// Each handler has an independent engine channel, matching the way transport
/// clients register separate subscriptions.
const FANOUT_HANDLERS: [u64; 3] = [1, 16, 64];

/// Completions measured per Criterion iteration.
///
/// This batch remains fixed even when Criterion adjusts `iters`.
const FANOUT_BATCH: u64 = 8;

/// Benchmark: workflow-completion emitter fan-out.
///
/// The handlers are registered once before measurement. Each Criterion
/// iteration arms exactly `FANOUT_BATCH` IRQ tasks outside the measured region.
///
/// The measured region includes each `complete()` call and receipt of the
/// resulting workflow-completion event by every registered handler.
fn emitter_fanout(c: &mut Criterion) {
    let mut group = c.benchmark_group("emitter_fanout");

    group.throughput(Throughput::Elements(FANOUT_BATCH));

    group.sample_size(10);

    let runtime = tokio::runtime::Runtime::new().unwrap();

    let workflow = Workflow::from_yml(include_str!("./act.yml")).unwrap();

    for &handlers in &FANOUT_HANDLERS {
        group.bench_function(BenchmarkId::new("channels", handlers.to_string()), |b| {
            let workflow = workflow.clone();

            b.to_async(&runtime).iter_custom(move |iters| {
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

                    let (tx, mut rx) = mpsc::unbounded_channel::<()>();

                    let mut channels = Vec::with_capacity(handlers as usize);

                    // Register the fan-out width once. Channel
                    // registration is setup, not part of the measured
                    // completion cost.
                    for _ in 0..handlers {
                        let chan = engine.channel();

                        let tx = tx.clone();

                        chan.on_complete(move |event| {
                            let tx = tx.clone();

                            async move {
                                if event.is_type("workflow")
                                    && event.is_state(MessageState::Completed)
                                {
                                    let _ = tx.send(());
                                }
                            }
                        });

                        channels.push(chan);
                    }

                    drop(tx);

                    let mut elapsed = Duration::ZERO;

                    // Keep the armed batch fixed. Criterion's `iters`
                    // controls how many batches are measured, not how
                    // many tasks are prepared simultaneously.
                    for _ in 0..iters {
                        let (tasks, _) = arm_batch(&engine, &workflow, FANOUT_BATCH).await;

                        let start = Instant::now();

                        for (pid, tid) in &tasks {
                            engine
                                .executor(&acts::Principal::unrestricted())
                                .act()
                                .complete(pid, tid, Vars::new())
                                .await
                                .unwrap();

                            // One workflow-completion event must be
                            // observed by every registered channel
                            // before the next completion is measured.
                            for _ in 0..handlers {
                                tokio::time::timeout(ARM_TIMEOUT, rx.recv())
                                    .await
                                    .expect("timed out waiting for fan-out handler")
                                    .expect("fan-out handler dropped");
                            }
                        }

                        elapsed += start.elapsed();
                    }

                    for chan in &channels {
                        chan.close();
                    }

                    engine.close().await;

                    elapsed
                }
            });
        });
    }

    group.finish();
}

criterion_group!(benches, expr_eval, scheduler_multi_pid, emitter_fanout,);

criterion_main!(benches);
