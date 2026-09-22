//! Latency stress harness — per-scenario p50/p95/p99 for the same engine and
//! store hotspots that `benches/store.rs`, `benches/engine.rs` and
//! `benches/model.rs` time with criterion.
//!
//! Criterion reports the mean estimate (and the median internally) and nothing
//! above the median, so the tail a caller actually budgets for has no output
//! there. This target measures the identical hotspots but keeps every
//! individual operation's duration and prints nearest-rank p50/p95/p99 (plus
//! max and mean) per scenario.
//!
//! Percentiles are nearest-rank over the sorted samples: `p` is the sample at
//! rank `ceil(p/100 * n)`, so `p99` of 200 samples is the second-largest
//! sample and `max` the largest. A scenario therefore needs around 100 samples
//! for its p99 to say anything; each one warms up untimed before it measures.
//!
//! The engine scenarios size the process cache to the batch they arm: under
//! the default `cache_cap` (1024) an oversized armed set is *parked* — never
//! started, so it emits nothing — and the sample would time out instead of
//! measuring.

use acts::{
    ActSchema, Engine, MemoryStore, MessageState, Store, Variant, VariantTypes, Vars, Workflow,
    data::Proc,
    query::{Expr, Filter, Query},
};
use parking_lot::Mutex;
use serde_json::{Value as JsonValue, json};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

/// Hard cap for arming one batch or draining one fan-out — a missing event has
/// to fail the run instead of blocking it forever.
const ARM_TIMEOUT: Duration = Duration::from_secs(30);

fn now_micros() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_micros() as i64
}

fn memory_store() -> Arc<Store> {
    Arc::new(Store::new(Arc::new(MemoryStore::new())))
}

fn make_proc(id: &str, mid: &str, i: u64) -> Proc {
    Proc {
        id: id.to_string(),
        name: format!("bench-{}", i),
        mid: mid.to_string(),
        state: "running".to_string(),
        start_time: 0,
        end_time: 0,
        timestamp: now_micros(),
        model: "{}".to_string(),
        env: "{}".to_string(),
        err: None,
        removable: false,
        v: 0,
    }
}

/// Measured durations of one scenario plus its label.
struct Report {
    name: String,
    samples: Vec<Duration>,
}

impl Report {
    fn new(name: String) -> Self {
        Self {
            name,
            samples: Vec::new(),
        }
    }

    fn push(&mut self, sample: Duration) {
        self.samples.push(sample);
    }

    /// Nearest-rank percentile over the sorted samples (see the module docs).
    fn percentile(sorted: &[Duration], p: f64) -> Duration {
        let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
        sorted[rank.clamp(1, sorted.len()) - 1]
    }

    fn print(&self) {
        let mut sorted = self.samples.clone();
        sorted.sort_unstable();
        let mean = sorted.iter().sum::<Duration>() / sorted.len() as u32;

        println!(
            "{:<34} {:>6} {:>10} {:>10} {:>10} {:>10} {:>10}",
            self.name,
            sorted.len(),
            fmt_dur(Self::percentile(&sorted, 50.0)),
            fmt_dur(Self::percentile(&sorted, 95.0)),
            fmt_dur(Self::percentile(&sorted, 99.0)),
            fmt_dur(*sorted.last().unwrap()),
            fmt_dur(mean),
        );
    }
}

fn fmt_dur(d: Duration) -> String {
    let us = d.as_secs_f64() * 1e6;
    if us >= 1000.0 {
        format!("{:.2}ms", us / 1000.0)
    } else {
        format!("{:.2}µs", us)
    }
}

fn print_header() {
    println!(
        "nearest-rank percentiles of the per-operation latency; p99 needs ~100 samples to be meaningful\n"
    );
    println!(
        "{:<34} {:>6} {:>10} {:>10} {:>10} {:>10} {:>10}",
        "scenario", "n", "p50", "p95", "p99", "max", "mean"
    );
}

/// Start an engine whose resident set fits the armed batch (see the module
/// docs for why the default cap is not enough here).
async fn engine_with_cache(cache_cap: i64) -> Engine {
    Engine::builder()
        .cache_size(cache_cap)
        .start()
        .await
        .expect("failed to start engine")
}

async fn deploy(engine: &Engine, workflow: &Workflow) {
    engine
        .executor(&acts::Principal::unrestricted())
        .model()
        .deploy(workflow, None)
        .await
        .unwrap();
}

/// Start `count` processes and wait until every `acts.core.irq` task reports
/// `Created`, returning the `(pid, tid)` pairs and the start→armed duration.
///
/// This is the untimed preparation of the act scenarios; the wait is bounded
/// so a batch that never reports fails the run instead of hanging.
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

/// Per-query latency of a selective `Eq` on the indexed `mid`.
async fn store_eq_index(rows: usize, n: usize) -> Report {
    const WARMUP: usize = 100;

    let store = memory_store();
    let procs = store.procs();
    let mut ids = Vec::with_capacity(rows);
    for i in 0..rows {
        let id = format!("lqi_{}_{}", rows, i);
        let mid = format!("m_lqi_{}_{}", rows, i);
        procs.create(&make_proc(&id, &mid, i as u64)).await.unwrap();
        ids.push(id);
    }

    let q = Query::new().filter(Filter::and().expr(Expr::eq("mid", format!("m_lqi_{}_0", rows))));
    let mut report = Report::new(format!("store/eq_index/rows={rows}"));
    for i in 0..(WARMUP + n) {
        let start = Instant::now();
        let _ = procs.query(&q).await.unwrap();
        let elapsed = start.elapsed();
        if i >= WARMUP {
            report.push(elapsed);
        }
    }

    for id in &ids {
        procs.delete(id).await.unwrap();
    }
    report
}

/// Per-query latency of the same `Eq` on the non-indexed `name`: every row is
/// read and its `Expr` evaluated.
async fn store_eq_scan(rows: usize, n: usize) -> Report {
    const WARMUP: usize = 20;

    let store = memory_store();
    let procs = store.procs();
    let mid = format!("m_lqs_{}", rows);
    let mut ids = Vec::with_capacity(rows);
    for i in 0..rows {
        let id = format!("lqs_{}_{}", rows, i);
        procs.create(&make_proc(&id, &mid, i as u64)).await.unwrap();
        ids.push(id);
    }

    let q = Query::new().filter(Filter::and().expr(Expr::eq("name", "bench-0")));
    let mut report = Report::new(format!("store/eq_scan/rows={rows}"));
    for i in 0..(WARMUP + n) {
        let start = Instant::now();
        let _ = procs.query(&q).await.unwrap();
        let elapsed = start.elapsed();
        if i >= WARMUP {
            report.push(elapsed);
        }
    }

    for id in &ids {
        procs.delete(id).await.unwrap();
    }
    report
}

/// A schema of `fields` typed required properties plus a matching instance.
/// `nonce` is part of every property name, so a distinct nonce is a distinct
/// serialized schema — a validator-cache miss.
fn schema_with(fields: usize, nonce: u64) -> (ActSchema, JsonValue) {
    let mut variants = Vec::with_capacity(fields);
    let mut data = serde_json::Map::with_capacity(fields);

    for i in 0..fields {
        let name = format!("f_{}_{}", nonce, i);
        variants.push(
            Variant::new()
                .name(&name)
                .r#type(VariantTypes::Number)
                .required(true),
        );
        data.insert(name, json!(i));
    }

    (ActSchema::Multiple(variants), JsonValue::Object(data))
}

/// Per-validation latency against a validator that is already compiled — the
/// production path, where a deployed workflow's inputs/exposes hit the cache.
fn schema_validate_cached(fields: usize, n: usize) -> Report {
    const WARMUP: usize = 500;

    let (schema, value) = schema_with(fields, 0);
    let mut report = Report::new(format!("schema/validate_cached/fields={fields}"));
    for i in 0..(WARMUP + n) {
        let start = Instant::now();
        schema.validate(&value).unwrap();
        let elapsed = start.elapsed();
        if i >= WARMUP {
            report.push(elapsed);
        }
    }
    report
}

/// Per-validation latency on a schema whose content was never seen before, so
/// the `VALIDATORS` cache misses and `jsonschema::Validator::new` runs. Each
/// sample caches one more validator; the cache's capacity bound keeps that
/// growth from outliving the measurement.
fn schema_compile(fields: usize, n: usize) -> Report {
    const WARMUP: usize = 50;

    let mut report = Report::new(format!("schema/compile/fields={fields}"));
    for i in 0..(WARMUP + n) {
        let (schema, value) = schema_with(fields, i as u64 + 1);
        let start = Instant::now();
        schema.validate(&value).unwrap();
        let elapsed = start.elapsed();
        if i >= WARMUP {
            report.push(elapsed);
        }
    }
    report
}

/// A workflow whose `acts.core.irq` params carry `exprs` `${{ ... }}`
/// expressions, evaluated while the act params are filled.
///
/// The expression is the same arithmetic the criterion bench's scenario uses —
/// the loop's index scaled — and it is one the engine's evaluator actually
/// supports. It read `Math.sqrt(i) * 1000` while QuickJS was the evaluator, and
/// `Math` has been an unknown variable since CEL took over, so the scenario had
/// been timing a *failing* evaluation: `fill_params` printed the error and
/// filled every param with null.
fn expr_workflow(exprs: usize) -> Workflow {
    let mut params = String::from("      key: act1\n");
    for i in 0..exprs {
        params.push_str(&format!("      v{}: '${{{{ {} * 1000 }}}}'\n", i, i));
    }

    let text = format!(
        "id: expr_bench\nver: 0.1.0\nsteps:\n  - id: step1\n    uses: acts.core.irq\n    params:\n{params}"
    );
    Workflow::from_yml(&text).unwrap()
}
async fn expr_eval(exprs: usize, n: usize) -> Report {
    const WARMUP: usize = 20;

    let workflow = expr_workflow(exprs);
    let engine = engine_with_cache((WARMUP + n + 32) as i64).await;
    deploy(&engine, &workflow).await;

    let mut report = Report::new(format!("engine/expr_eval/exprs={exprs}"));
    for i in 0..(WARMUP + n) {
        let (mut tasks, elapsed) = arm_batch(&engine, &workflow, 1).await;
        // complete it (untimed) so the process is evicted between samples
        if let Some((pid, tid)) = tasks.pop() {
            engine
                .executor(&acts::Principal::unrestricted())
                .act()
                .complete(&pid, &tid, Vars::new())
                .await
                .unwrap();
        }
        if i >= WARMUP {
            report.push(elapsed);
        }
    }

    engine.close().await;
    report
}

/// Workflow for the scheduler scenario: four no-op steps, so every process
/// runs its root task plus one task per step through the pid-hashed lanes.
const SCHED_WORKFLOW: &str = r#"
id: sched_bench
ver: 0.1.0
steps:
  - id: s1
  - id: s2
  - id: s3
  - id: s4
"#;

/// Per-process submit→done latency inside a burst of `batch` callers that all
/// submit at once: the sample is the time from the burst's first `start` to
/// each process's workflow-complete event, so the spread across the burst is
/// the queueing the `workers` lanes impose on callers that arrive together.
///
/// `batch` calls are all in flight at once — a caller's own `start` cost is
/// therefore part of the burst's load rather than a serial prologue, which is
/// what makes the lane count visible in the percentiles.
async fn proc_start(workers: usize, batch: usize, bursts: usize) -> Report {
    let workflow = Workflow::from_yml(SCHED_WORKFLOW).unwrap();
    let engine = Engine::builder()
        .scheduler_workers(workers)
        .cache_size((batch * 2) as i64)
        .start()
        .await
        .expect("failed to start engine");
    deploy(&engine, &workflow).await;

    let (tx, mut rx) = mpsc::unbounded_channel::<()>();
    let chan = engine.channel();
    {
        let tx = tx.clone();
        chan.on_complete(move |e| {
            let tx = tx.clone();
            async move {
                if e.is_type("workflow") && e.is_state(MessageState::Completed) {
                    let _ = tx.send(());
                }
            }
        });
    }
    drop(tx);

    let mut report = Report::new(format!("engine/proc_start/workers={workers}"));
    for burst in 0..bursts {
        // every caller of the burst submits at the same instant
        let burst_start = Instant::now();
        let mut calls = Vec::with_capacity(batch);
        for _ in 0..batch {
            let engine = engine.clone();
            let mid = workflow.id.clone();
            calls.push(tokio::spawn(async move {
                engine
                    .executor(&acts::Principal::unrestricted())
                    .proc()
                    .start(&mid, Vars::new())
                    .await
                    .unwrap();
            }));
        }
        for call in calls {
            call.await.unwrap();
        }

        for _ in 0..batch {
            tokio::time::timeout(ARM_TIMEOUT, rx.recv())
                .await
                .expect("timed out waiting for a process completion")
                .expect("completion channel closed");
            if burst > 0 {
                report.push(burst_start.elapsed());
            }
        }
    }

    chan.close();
    engine.close().await;
    report
}

/// Per-completion latency with `handlers` registered channels: the sample
/// covers `act().complete()` plus every channel handler's receipt of the
/// resulting workflow-complete event, which is the fan-out the emitter pays
/// per emitted message.
async fn act_complete(handlers: usize, n: usize) -> Report {
    const WARMUP: usize = 50;

    let workflow = Workflow::from_yml(include_str!("./act.yml")).unwrap();
    let engine = engine_with_cache((WARMUP + n + 32) as i64).await;
    deploy(&engine, &workflow).await;

    // every completion is armed outside the timed region
    let (tasks, _) = arm_batch(&engine, &workflow, (WARMUP + n) as u64).await;

    let (tx, mut rx) = mpsc::unbounded_channel::<()>();
    let mut channels = Vec::with_capacity(handlers);
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

    let mut report = Report::new(format!("engine/act_complete/handlers={handlers}"));
    for (i, (pid, tid)) in tasks.iter().enumerate() {
        let start = Instant::now();
        engine
            .executor(&acts::Principal::unrestricted())
            .act()
            .complete(pid, tid, Vars::new())
            .await
            .unwrap();
        for _ in 0..handlers {
            tokio::time::timeout(ARM_TIMEOUT, rx.recv())
                .await
                .expect("timed out waiting for a workflow-complete delivery")
                .expect("fan-out channel closed");
        }
        if i >= WARMUP {
            report.push(start.elapsed());
        }
    }

    for chan in &channels {
        chan.close();
    }
    engine.close().await;
    report
}

#[tokio::main]
async fn main() {
    print_header();

    store_eq_index(100, 2000).await.print();
    store_eq_index(1000, 1000).await.print();
    store_eq_scan(100, 500).await.print();
    store_eq_scan(1000, 200).await.print();

    schema_validate_cached(32, 5000).print();
    schema_compile(8, 500).print();

    expr_eval(16, 200).await.print();

    proc_start(1, 32, 25).await.print();
    proc_start(4, 32, 25).await.print();
    act_complete(1, 1000).await.print();
    // the same sample count for both: the resident set (and with it the
    // per-completion store scans) sizes with the armed batch, so a different
    // n per variant would compare two different engines, not two fan-out
    // widths
    act_complete(16, 1000).await.print();
}
