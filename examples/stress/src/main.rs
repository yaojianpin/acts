//! End-to-end million-run stress test for the Acts workflow engine.
//!
//! Run from the workspace root:
//!   cargo run --release -p acts --example stress -- --count 1000000
//!
//! This is a closed-loop load generator: each worker starts one workflow and
//! waits for its durable terminal outcome before starting the next one. It
//! intentionally uses RunHandle::wait rather than event notifications, so a
//! fast workflow cannot finish before a listener subscribes.

use acts::{
    CancellationToken, CpuStatus, Engine, HealthStatus, KvStore, MemoryStatus, MemoryStore,
    Principal, ProcessCounts, RunHandle, RunOutcome, StorageStatus, SystemStatus, Vars, Workflow,
};
use acts_store::SledStore;
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tracing::{debug, info, trace, warn};

const DEFAULT_COUNT: u64 = 1_000_000;
const DEFAULT_SOAK_SECONDS: u64 = 30;
const DEFAULT_TIMEOUT_SECONDS: u64 = 120;
const DEFAULT_SAMPLE_MS: u64 = 1_000;
const DEFAULT_ERROR_LIMIT_PCT: f64 = 0.1;

const STRESS_WORKFLOW: &str = r#"
id: stress_million
name: million-run stress workload
ver: 1.0.0
vars:
  - name: input
    value: 5
  - name: index
    value: 0
  - name: value
    value: 0
exposes:
  - name: result
    value: ${{ value }}
steps:
  - id: init
    uses: acts.transform.set
    params:
      value: 0
      index: 0
  - id: loop
    while: index <= input
    uses: acts.transform.set
    params:
      value: '${{ value + index }}'
      index: '${{ index + 1 }}'
  - id: branch
    branches:
      - id: high
        if: value > 10
        steps:
          - id: high_a
            uses: acts.transform.set
            params:
              result_kind: high
              checksum: '${{ value * 17 }}'
      - id: low
        if: value <= 10
        steps:
          - id: low_a
            uses: acts.transform.set
            params:
              result_kind: low
              checksum: '${{ value * 19 }}'
  - id: finalize
    uses: acts.transform.set
    params:
      finished: true
"#;

#[derive(Debug, Clone)]
struct Options {
    count: u64,
    concurrency: Vec<usize>,
    warmup: u64,
    soak_seconds: u64,
    sample_ms: u64,
    timeout_seconds: u64,
    scheduler_workers: usize,
    store_workers: usize,
    /// `sled` (durable, `./data`) or `memory` (Engine default). Memory
    /// isolates engine scheduling and effect cost from storage fsync cost.
    store: String,
    output: PathBuf,
    /// `EnvFilter` directives for tracing. `$ACTS_LOG` is the default (like
    /// acts-server); `--log-level` overrides it.
    log_level: String,
    max_error_rate_pct: f64,
}

impl Options {
    fn parse() -> Result<Self, String> {
        let cores = std::thread::available_parallelism()
            .map(|value| value.get())
            .unwrap_or(4)
            .max(1);
        let base = cores.clamp(4, 32);
        let mut options = Self {
            count: DEFAULT_COUNT,
            concurrency: unique_sorted(vec![base, base * 2, base * 4]),
            warmup: 500,
            soak_seconds: DEFAULT_SOAK_SECONDS,
            sample_ms: DEFAULT_SAMPLE_MS,
            timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
            scheduler_workers: cores,
            store_workers: cores,
            store: "sled".to_string(),
            output: PathBuf::from("target/stress-results"),
            log_level: env::var("ACTS_LOG").unwrap_or_else(|_| "info".to_string()),
            max_error_rate_pct: DEFAULT_ERROR_LIMIT_PCT,
        };

        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            let value = |args: &mut std::iter::Skip<std::env::Args>, name: &str| {
                args.next()
                    .ok_or_else(|| format!("missing value after {name}"))
            };
            match arg.as_str() {
                "--count" => options.count = parse_num(&value(&mut args, "--count")?, "count")?,
                "--concurrency" => {
                    options.concurrency = parse_list(&value(&mut args, "--concurrency")?)?;
                }
                "--warmup" => options.warmup = parse_num(&value(&mut args, "--warmup")?, "warmup")?,
                "--soak-seconds" => {
                    options.soak_seconds =
                        parse_num(&value(&mut args, "--soak-seconds")?, "soak-seconds")?;
                }
                "--sample-ms" => {
                    options.sample_ms = parse_num(&value(&mut args, "--sample-ms")?, "sample-ms")?;
                }
                "--timeout-seconds" => {
                    options.timeout_seconds =
                        parse_num(&value(&mut args, "--timeout-seconds")?, "timeout-seconds")?;
                }
                "--workers" => {
                    options.scheduler_workers =
                        parse_num(&value(&mut args, "--workers")?, "workers")?;
                }
                "--store-workers" => {
                    options.store_workers =
                        parse_num(&value(&mut args, "--store-workers")?, "store-workers")?;
                }
                "--store" => options.store = value(&mut args, "--store")?,
                "--output" => options.output = PathBuf::from(value(&mut args, "--output")?),
                "--log-level" => options.log_level = value(&mut args, "--log-level")?,
                "--max-error-rate-pct" => {
                    options.max_error_rate_pct = value(&mut args, "--max-error-rate-pct")?
                        .parse::<f64>()
                        .map_err(|e| format!("invalid max-error-rate-pct: {e}"))?;
                }
                "--help" | "-h" => {
                    print_help();
                    std::process::exit(0);
                }
                other => return Err(format!("unknown argument: {other}\nUse --help for usage.")),
            }
        }

        if options.count == 0 {
            return Err("--count must be greater than zero".into());
        }
        if options.concurrency.is_empty() || options.concurrency.contains(&0) {
            return Err("--concurrency requires positive integers".into());
        }
        if options.count < options.concurrency.len() as u64 {
            return Err("--count must be at least the number of concurrency stages".into());
        }
        if options.warmup == 0 || options.sample_ms == 0 || options.timeout_seconds == 0 {
            return Err("warmup, sample-ms and timeout-seconds must be greater than zero".into());
        }
        if options.scheduler_workers == 0 || options.store_workers == 0 {
            return Err("workers and store-workers must be greater than zero".into());
        }
        if options.scheduler_workers > 1_024 || options.store_workers > 1_024 {
            return Err("workers and store-workers cannot exceed 1024".into());
        }
        if options.concurrency.iter().any(|n| *n > 8_192) {
            return Err("a concurrency stage cannot exceed 8192 workers".into());
        }
        if !matches!(options.store.as_str(), "memory" | "sled") {
            return Err("--store must be 'sled' or 'memory'".into());
        }
        if options.log_level.trim().is_empty() {
            return Err("--log-level cannot be empty".into());
        }
        if !options.max_error_rate_pct.is_finite() || options.max_error_rate_pct < 0.0 {
            return Err("max-error-rate-pct must be a finite non-negative number".into());
        }
        options.concurrency = unique_sorted(options.concurrency.clone());
        Ok(options)
    }
}

fn print_help() {
    println!(
        "Acts million-run stress test\n\n\
         USAGE:\n  cargo run --release -p acts-example-stress -- [OPTIONS]\n\n\
         OPTIONS:\n\
           --count N                 Measured workflows, default 1000000\n\
           --concurrency A,B,C       Concurrency ramp, default based on CPU cores\n\
           --warmup N                Warm-up workflows, default 500\n\
           --soak-seconds N          Extra max-concurrency soak, default 30 (0 disables)\n\
           --sample-ms N             System metric sampling interval, default 1000\n\
           --timeout-seconds N       Per-workflow completion timeout, default 120\n\
           --workers N               Scheduler lanes, default logical CPU count\n\
           --store-workers N         Store writer shards, default logical CPU count\n\
           --store BACKEND           sled (default, ./data) or memory\n\
           --output DIR               Output directory, default target/stress-results\n\
           --log-level FILTER         Tracing filter, default $ACTS_LOG or 'info'\n\
           --max-error-rate-pct N    Error-rate gate, default 0.1 percent\n\
           -h, --help                Show this help\n\n\
         Logging:\n\
           A trace.log is written under --output and mirrored to stdout. Use\n\
           --log-level, or ACTS_LOG, with EnvFilter directives to inspect\n\
           engine internals, for example:\n\
             ACTS_LOG=acts=debug,acts_example_stress=debug\n\
           `acts=trace` adds one 'scheduler lane executing operation' line per\n\
           operation: count them per pid to measure transitions/workflow.\n\n\
         Example:\n\
           cargo run --release -p acts-example-stress -- --count 1000000 --concurrency 64,128,256 --soak-seconds 60 --log-level acts=debug,acts_example_stress=debug"
    );
}

fn parse_num<T: std::str::FromStr>(value: &str, name: &str) -> Result<T, String>
where
    T::Err: std::fmt::Display,
{
    value.parse().map_err(|e| format!("invalid {name}: {e}"))
}

fn parse_list(value: &str) -> Result<Vec<usize>, String> {
    let values = value
        .split(',')
        .map(|part| parse_num::<usize>(part.trim(), "concurrency"))
        .collect::<Result<Vec<_>, _>>()?;
    if values.contains(&0) {
        return Err("concurrency values must be greater than zero".into());
    }
    Ok(unique_sorted(values))
}

fn unique_sorted(mut values: Vec<usize>) -> Vec<usize> {
    values.sort_unstable();
    values.dedup();
    values
}

#[derive(Debug, Default)]
struct WorkerResult {
    attempted: u64,
    completed: u64,
    errored: u64,
    terminated: u64,
    start_latency_ns: Vec<u64>,
    end_to_end_latency_ns: Vec<u64>,
    first_errors: Vec<String>,
    timed_out: bool,
}

impl WorkerResult {
    fn record_error(&mut self, message: String) {
        self.errored += 1;
        if self.first_errors.len() < 8 {
            self.first_errors.push(message);
        }
    }

    fn merge(&mut self, mut other: Self) {
        self.attempted += other.attempted;
        self.completed += other.completed;
        self.errored += other.errored;
        self.terminated += other.terminated;
        self.start_latency_ns.append(&mut other.start_latency_ns);
        self.end_to_end_latency_ns
            .append(&mut other.end_to_end_latency_ns);
        self.first_errors.append(&mut other.first_errors);
        self.first_errors.truncate(8);
        self.timed_out |= other.timed_out;
    }
}

#[derive(Debug, Clone, Serialize)]
struct LatencyStats {
    samples: usize,
    min_ms: f64,
    mean_ms: f64,
    stddev_ms: f64,
    p50_ms: f64,
    p90_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    p999_ms: f64,
    max_ms: f64,
}

impl LatencyStats {
    fn from_ns(values: &[u64]) -> Option<Self> {
        if values.is_empty() {
            return None;
        }
        let mut sorted = values.to_vec();
        sorted.sort_unstable();
        let mean_ns = sorted.iter().map(|v| *v as f64).sum::<f64>() / sorted.len() as f64;
        let variance = sorted
            .iter()
            .map(|v| {
                let delta = *v as f64 - mean_ns;
                delta * delta
            })
            .sum::<f64>()
            / sorted.len() as f64;

        let pick = |p: f64| -> f64 {
            let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
            sorted[rank.clamp(1, sorted.len()) - 1] as f64 / 1_000_000.0
        };
        Some(Self {
            samples: sorted.len(),
            min_ms: *sorted.first().unwrap() as f64 / 1_000_000.0,
            mean_ms: mean_ns / 1_000_000.0,
            stddev_ms: variance.sqrt() / 1_000_000.0,
            p50_ms: pick(50.0),
            p90_ms: pick(90.0),
            p95_ms: pick(95.0),
            p99_ms: pick(99.0),
            p999_ms: pick(99.9),
            max_ms: *sorted.last().unwrap() as f64 / 1_000_000.0,
        })
    }
}

#[derive(Debug, Serialize)]
struct PhaseReport {
    name: String,
    concurrency: usize,
    target: Option<u64>,
    attempted: u64,
    completed: u64,
    errored: u64,
    terminated: u64,
    elapsed_seconds: f64,
    workflows_per_second: f64,
    error_rate_pct: f64,
    start_latency: Option<LatencyStats>,
    end_to_end_latency: Option<LatencyStats>,
    first_errors: Vec<String>,
    timed_out: bool,
}

#[derive(Debug, Clone, Serialize)]
struct MetricPoint {
    elapsed_seconds: f64,
    phase: String,
    concurrency: usize,
    memory: MemoryStatus,
    cpu: CpuStatus,
    storage: StorageStatus,
    processes: ProcessCounts,
    health: HealthStatus,
}

impl MetricPoint {
    fn new(elapsed_seconds: f64, phase: String, concurrency: usize, status: SystemStatus) -> Self {
        // Deliberately omit SystemStatus.config: a user's config table may
        // contain credentials, and load reports must never copy those values.
        Self {
            elapsed_seconds,
            phase,
            concurrency,
            memory: status.memory,
            cpu: status.cpu,
            storage: status.storage,
            processes: status.processes,
            health: status.health,
        }
    }
}

#[derive(Debug, Serialize)]
struct ConfigReport {
    measured_workflows: u64,
    concurrency_stages: Vec<usize>,
    warmup_workflows: u64,
    soak_seconds: u64,
    scheduler_workers: usize,
    store_writer_workers: usize,
    cache_cap: usize,
    scheduler_queue_cap: usize,
    store_writer_queue_cap: usize,
    sample_interval_ms: u64,
    workflow_timeout_seconds: u64,
    engine_version: String,
    store_backend: String,
    max_error_rate_pct: f64,
    workload: String,
    host_os: String,
    host_arch: String,
    logical_cores: usize,
    note: String,
}

#[derive(Debug, Serialize)]
struct FinalReport {
    generated_at_unix_seconds: u64,
    status: String,
    total_attempted: u64,
    total_completed: u64,
    total_errored: u64,
    total_terminated: u64,
    overall_error_rate_pct: f64,
    elapsed_seconds: f64,
    workflows_per_second: f64,
    max_rss_bytes: u64,
    min_system_available_bytes: u64,
    max_process_cpu_percent: f32,
    avg_process_cpu_percent: f32,
    max_system_cpu_percent: f32,
    avg_system_cpu_percent: f32,
    max_storage_used_bytes: Option<u64>,
    max_process_rows: usize,
    max_active_processes: usize,
    logical_cores: usize,
    degraded_health_samples: usize,
    phases: Vec<PhaseReport>,
    metrics: Vec<MetricPoint>,
    metrics_collection_errors: u64,
    config: ConfigReport,
    shutdown_seconds: f64,
}

async fn run_phase(
    executor: &Arc<acts::Executor>,
    workflow_id: &str,
    name: String,
    concurrency: usize,
    target: Option<u64>,
    duration: Option<Duration>,
    timeout_seconds: u64,
) -> PhaseReport {
    let started = Instant::now();
    let next = Arc::new(AtomicU64::new(0));
    let abort = Arc::new(AtomicBool::new(false));
    let deadline = duration.map(|d| Instant::now() + d);
    let timeout = Duration::from_secs(timeout_seconds);
    let mut workers = Vec::with_capacity(concurrency);
    info!(
        phase = %name,
        concurrency,
        target = ?target,
        duration = ?duration,
        timeout_seconds,
        "phase starting"
    );

    for worker_index in 0..concurrency {
        let next = next.clone();
        let abort = abort.clone();
        let executor = executor.clone();
        let workflow_id = workflow_id.to_string();
        let phase = name.clone();
        workers.push(tokio::spawn(async move {
            let mut result = WorkerResult::default();
            debug!(phase = %phase, worker = worker_index, "phase worker started");
            loop {
                if abort.load(Ordering::Relaxed) {
                    break;
                }
                if deadline.is_some_and(|until| Instant::now() >= until) {
                    break;
                }
                let index = next.fetch_add(1, Ordering::Relaxed);
                if target.is_some_and(|limit| index >= limit) {
                    break;
                }

                let request_started = Instant::now();
                let run_vars = Vars::new().with("input", index % 10);
                let started_run = executor.proc().start(&workflow_id, run_vars).await;
                let start_latency = request_started.elapsed();
                result
                    .start_latency_ns
                    .push(start_latency.as_nanos().min(u64::MAX as u128) as u64);
                result.attempted += 1;
                let handle: RunHandle = match started_run {
                    Ok(handle) => handle,
                    Err(error) => {
                        warn!(
                            phase = %phase,
                            worker = worker_index,
                            item = index,
                            error = %error,
                            "proc:start failed"
                        );
                        result.record_error(format!("proc:start failed at item {index}: {error}"));
                        continue;
                    }
                };

                match tokio::time::timeout(timeout, handle.wait()).await {
                    Ok(Ok(RunOutcome::Completed)) => {
                        result.completed += 1;
                        let elapsed = request_started.elapsed();
                        trace!(
                            phase = %phase,
                            worker = worker_index,
                            item = index,
                            pid = %handle.pid(),
                            start_ms = start_latency.as_secs_f64() * 1_000.0,
                            end_to_end_ms = elapsed.as_secs_f64() * 1_000.0,
                            "workflow completed"
                        );
                        result
                            .end_to_end_latency_ns
                            .push(elapsed.as_nanos().min(u64::MAX as u128) as u64);
                    }
                    Ok(Ok(RunOutcome::Errored { code, message })) => {
                        warn!(
                            phase = %phase,
                            worker = worker_index,
                            item = index,
                            pid = %handle.pid(),
                            code = %code,
                            error = %message,
                            "workflow errored"
                        );
                        result.record_error(format!(
                            "run {} errored ({code}): {message}",
                            handle.pid()
                        ));
                        result.end_to_end_latency_ns.push(
                            request_started.elapsed().as_nanos().min(u64::MAX as u128) as u64,
                        );
                    }
                    Ok(Ok(RunOutcome::Terminated)) => {
                        debug!(
                            phase = %phase,
                            worker = worker_index,
                            item = index,
                            pid = %handle.pid(),
                            "workflow terminated"
                        );
                        result.terminated += 1;
                        result.end_to_end_latency_ns.push(
                            request_started.elapsed().as_nanos().min(u64::MAX as u128) as u64,
                        );
                    }
                    Ok(Err(error)) => {
                        warn!(
                            phase = %phase,
                            worker = worker_index,
                            item = index,
                            pid = %handle.pid(),
                            error = %error,
                            "waiting for the run failed"
                        );
                        result.record_error(format!("wait {} failed: {error}", handle.pid()));
                        result.end_to_end_latency_ns.push(
                            request_started.elapsed().as_nanos().min(u64::MAX as u128) as u64,
                        );
                    }
                    Err(_) => {
                        warn!(
                            phase = %phase,
                            worker = worker_index,
                            item = index,
                            pid = %handle.pid(),
                            timeout = ?timeout,
                            "workflow timed out"
                        );
                        result.timed_out = true;
                        result.record_error(format!(
                            "workflow {} timed out after {timeout:?}",
                            handle.pid()
                        ));
                        result.end_to_end_latency_ns.push(
                            request_started.elapsed().as_nanos().min(u64::MAX as u128) as u64,
                        );
                        abort.store(true, Ordering::Relaxed);
                        break;
                    }
                }
            }
            debug!(
                phase = %phase,
                worker = worker_index,
                attempted = result.attempted,
                completed = result.completed,
                errors = result.errored,
                terminated = result.terminated,
                timed_out = result.timed_out,
                "phase worker finished"
            );
            result
        }));
    }

    let mut aggregate = WorkerResult::default();
    for worker in workers {
        match worker.await {
            Ok(result) => aggregate.merge(result),
            Err(error) => {
                warn!(phase = %name, error = %error, "phase worker task failed to join");
                aggregate.record_error(format!("worker task join failed: {error}"));
            }
        }
    }
    let elapsed = started.elapsed().as_secs_f64();
    let attempted = aggregate.attempted;
    let errors = aggregate.errored + aggregate.terminated;
    let report = PhaseReport {
        name,
        concurrency,
        target,
        attempted,
        completed: aggregate.completed,
        errored: aggregate.errored,
        terminated: aggregate.terminated,
        elapsed_seconds: elapsed,
        workflows_per_second: if elapsed > 0.0 {
            aggregate.completed as f64 / elapsed
        } else {
            0.0
        },
        error_rate_pct: if attempted > 0 {
            errors as f64 * 100.0 / attempted as f64
        } else {
            0.0
        },
        start_latency: LatencyStats::from_ns(&aggregate.start_latency_ns),
        end_to_end_latency: LatencyStats::from_ns(&aggregate.end_to_end_latency_ns),
        first_errors: aggregate.first_errors,
        timed_out: aggregate.timed_out,
    };
    info!(
        phase = %report.name,
        concurrency = report.concurrency,
        attempted = report.attempted,
        completed = report.completed,
        errored = report.errored,
        terminated = report.terminated,
        elapsed_seconds = report.elapsed_seconds,
        workflows_per_second = report.workflows_per_second,
        error_rate_pct = report.error_rate_pct,
        p99_ms = report.end_to_end_latency.as_ref().map_or(0.0, |s| s.p99_ms),
        timed_out = report.timed_out,
        "phase finished"
    );
    report
}

async fn collect_status(engine: &Engine, principal: &Principal) -> Result<SystemStatus, String> {
    let value = acts::actions::apply_as(engine, principal, "sys:status", Vars::new())
        .await
        .map_err(|error| error.to_string())?;
    serde_json::from_value(value).map_err(|error| format!("invalid sys:status payload: {error}"))
}

struct MetricsSampler {
    engine: Engine,
    principal: Principal,
    stop: CancellationToken,
    interval: Duration,
    origin: Instant,
    phase: Arc<RwLock<(String, usize)>>,
    samples: Arc<Mutex<Vec<MetricPoint>>>,
    collection_errors: Arc<AtomicU64>,
}

impl MetricsSampler {
    async fn run(self) {
        loop {
            tokio::select! {
                _ = self.stop.cancelled() => break,
                _ = tokio::time::sleep(self.interval) => {
                    match collect_status(&self.engine, &self.principal).await {
                        Ok(status) => {
                            let current = self.phase.read().clone();
                            let elapsed_seconds = self.origin.elapsed().as_secs_f64();
                            let (phase_name, concurrency) = current;
                            debug!(
                                elapsed_seconds,
                                phase = %phase_name,
                                concurrency,
                                rss_bytes = status.memory.rss_bytes,
                                system_available_bytes = status.memory.system_available_bytes,
                                process_cpu_percent = status.cpu.process_percent,
                                system_cpu_percent = status.cpu.system_percent,
                                storage_used_bytes = ?status.storage.used_bytes,
                                processes_total = status.processes.total,
                                processes_active = status.processes.active,
                                health = %status.health.state,
                                "stress metrics sample"
                            );
                            self.samples.lock().push(MetricPoint::new(
                                elapsed_seconds,
                                phase_name,
                                concurrency,
                                status,
                            ));
                        }
                        Err(error) => {
                            self.collection_errors.fetch_add(1, Ordering::Relaxed);
                            warn!(error = %error, "status sampling failed");
                            eprintln!("[metrics] status sampling failed: {error}");
                        }
                    }
                }
            }
        }
    }
}

fn summarize_metrics(points: &[MetricPoint]) -> (u64, u64, f32, f32, usize) {
    let max_rss = points.iter().map(|p| p.memory.rss_bytes).max().unwrap_or(0);
    let min_available = points
        .iter()
        .map(|p| p.memory.system_available_bytes)
        .min()
        .unwrap_or(0);
    let max_proc_cpu = points
        .iter()
        .map(|p| p.cpu.process_percent)
        .fold(0.0_f32, f32::max);
    let max_sys_cpu = points
        .iter()
        .map(|p| p.cpu.system_percent)
        .fold(0.0_f32, f32::max);
    let degraded = points
        .iter()
        .filter(|p| p.health.state != "healthy")
        .count();
    (max_rss, min_available, max_proc_cpu, max_sys_cpu, degraded)
}

fn write_metrics_csv(path: &Path, points: &[MetricPoint]) -> std::io::Result<()> {
    let mut csv = String::from(
        "elapsed_seconds,phase,concurrency,process_rss_bytes,system_total_bytes,system_available_bytes,process_cpu_percent,system_cpu_percent,storage_kind,storage_used_bytes,processes_total,processes_active,health_state,degraded_loops\n",
    );
    for point in points {
        let degraded_loops = point
            .health
            .loops
            .iter()
            .filter(|item| item.state != "healthy")
            .map(|item| format!("{}:{}", item.name, item.consecutive_failures))
            .collect::<Vec<_>>()
            .join(";");
        csv.push_str(&format!(
            "{:.3},{},{},{},{},{},{:.2},{:.2},{},{},{},{},{},{}\n",
            point.elapsed_seconds,
            point.phase,
            point.concurrency,
            point.memory.rss_bytes,
            point.memory.system_total_bytes,
            point.memory.system_available_bytes,
            point.cpu.process_percent,
            point.cpu.system_percent,
            point.storage.kind,
            point
                .storage
                .used_bytes
                .map(|v| v.to_string())
                .unwrap_or_default(),
            point.processes.total,
            point.processes.active,
            point.health.state,
            degraded_loops,
        ));
    }
    fs::write(path, csv)
}

fn write_markdown(path: &Path, report: &FinalReport) -> std::io::Result<()> {
    let mut md = String::new();
    md.push_str("# Acts million-run stress report\n\n");
    md.push_str(&format!(
        "- Generated at (Unix seconds): {}\n- Result: **{}**\n- Measured attempts: {}\n- Measured completed: {}\n- Measured errored: {}\n- Measured terminated: {}\n- Measured error rate: {:.4}%\n- Measured elapsed: {:.2}s\n- Measured throughput: {:.2} workflows/s\n- Peak RSS: {:.2} GiB\n- Min system available memory: {:.2} GiB\n- Process CPU: peak {:.1}% / sample mean {:.1}%\n- System CPU: peak {:.1}% / sample mean {:.1}%\n- Max storage used: {}\n- Peak process rows: {} (active {})\n- Logical CPUs: {}\n- Degraded health samples: {}\n- Metric collection failures: {}\n- Engine shutdown: {:.3}s\n\n",
        report.generated_at_unix_seconds,
        report.status,
        report.total_attempted,
        report.total_completed,
        report.total_errored,
        report.total_terminated,
        report.overall_error_rate_pct,
        report.elapsed_seconds,
        report.workflows_per_second,
        report.max_rss_bytes as f64 / 1_073_741_824.0,
        report.min_system_available_bytes as f64 / 1_073_741_824.0,
        report.max_process_cpu_percent,
        report.avg_process_cpu_percent,
        report.max_system_cpu_percent,
        report.avg_system_cpu_percent,
        report.max_storage_used_bytes.map(|v| format!("{:.2} GiB", v as f64 / 1_073_741_824.0)).unwrap_or_else(|| "unknown".into()),
        report.max_process_rows,
        report.max_active_processes,
        report.logical_cores,
        report.degraded_health_samples,
        report.metrics_collection_errors,
        report.shutdown_seconds,
    ));
    md.push_str("## Test configuration\n\n");
    md.push_str(&format!(
        "- Workload: {}\n- Host: {} / {} ({} logical CPUs)\n- Concurrency stages: {:?}\n- Warmup workflows: {}\n- Steady-state soak: {}s\n- Scheduler workers: {}\n- Store writer workers: {}\n- Cache cap: {}\n- Scheduler queue cap: {}\n- Store writer queue cap: {}\n- Metric sample interval: {}ms (each status sample also waits for sysinfo's CPU delta window)\n- Per-workflow wait timeout: {}s\n- Engine version: {}\n- Store backend: {}\n- Error-rate gate: {:.3}%\n\n{}\n\n",
        report.config.workload,
        report.config.host_os,
        report.config.host_arch,
        report.config.logical_cores,
        report.config.concurrency_stages,
        report.config.warmup_workflows,
        report.config.soak_seconds,
        report.config.scheduler_workers,
        report.config.store_writer_workers,
        report.config.cache_cap,
        report.config.scheduler_queue_cap,
        report.config.store_writer_queue_cap,
        report.config.sample_interval_ms,
        report.config.workflow_timeout_seconds,
        report.config.engine_version,
        report.config.store_backend,
        report.config.max_error_rate_pct,
        report.config.note,
    ));
    md.push_str("## Per-phase results\n\n");
    md.push_str("| Phase | Concurrency | Attempted | Completed | Errored | Terminated | Throughput/s | Error rate | Elapsed/s |\n|---|---:|---:|---:|---:|---:|---:|---:|---:|\n");
    for phase in &report.phases {
        md.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {:.2} | {:.4}% | {:.2} |\n",
            phase.name,
            phase.concurrency,
            phase.attempted,
            phase.completed,
            phase.errored,
            phase.terminated,
            phase.workflows_per_second,
            phase.error_rate_pct,
            phase.elapsed_seconds,
        ));
        if let Some(stats) = &phase.start_latency {
            md.push_str(&format!(
                "\n**{} start API latency (ms)**: p50 {:.3} / p90 {:.3} / p95 {:.3} / p99 {:.3} / p99.9 {:.3} / max {:.3} / mean {:.3} / stddev {:.3}\n",
                phase.name, stats.p50_ms, stats.p90_ms, stats.p95_ms, stats.p99_ms,
                stats.p999_ms, stats.max_ms, stats.mean_ms, stats.stddev_ms,
            ));
        }
        if let Some(stats) = &phase.end_to_end_latency {
            md.push_str(&format!(
                "**{} end-to-end latency (ms)**: p50 {:.3} / p90 {:.3} / p95 {:.3} / p99 {:.3} / p99.9 {:.3} / max {:.3} / mean {:.3} / stddev {:.3}\n",
                phase.name, stats.p50_ms, stats.p90_ms, stats.p95_ms, stats.p99_ms,
                stats.p999_ms, stats.max_ms, stats.mean_ms, stats.stddev_ms,
            ));
        }
        if !phase.first_errors.is_empty() {
            md.push_str("\nFirst error samples:\n");
            for error in &phase.first_errors {
                md.push_str(&format!("- `{}`\n", error.replace('`', "'")));
            }
        }
        md.push('\n');
    }
    md.push_str("## Metric definitions and limitations\n\n");
    md.push_str("- Each run's `input` varies 0–9, so the while-loop length and both branches are exercised.\n- The report keeps status metrics only; it never copies `SystemStatus.config`, so credentials in configuration cannot leak into a report.\n- start API latency: from the `proc:start` call until a durable run handle is returned.\n- end-to-end latency: from submission until `RunHandle::wait` observes a terminal outcome, including queueing and execution.\n- workflows/s: completed runs divided by the phase's wall-clock time; errored and terminated runs do not count.\n- Percentiles use nearest-rank; stddev is the sample standard deviation.\n- CPU/RSS/storage/process counts/background loop health come from the engine's `sys:status`; each CPU sample waits for sysinfo's minimum CPU delta window, so the actual sampling interval is at least `sample-ms` and the sampler itself adds some overhead. Peaks are sample points, not continuous peak values. Collection failures are counted and fail the PASS gate.\n- This is a closed-loop load: each worker submits its next run only after the previous one finished; it is not a fixed-arrival-rate open-loop model.\n- The backend is selected by `--store` (SledStore at `./data` by default; `memory` isolates engine cost from storage cost). This does not replace benchmarks for SQLite/Postgres or claim durable crash-recovery coverage.\n- PASS means the measured target was submitted and every measured/soak/post-stress phase stayed under the error-rate gate, with no wait timeouts, a complete post-stress smoke run, healthy background loops and successful metric collection. A few failures under the gate are still reported. Use `--max-error-rate-pct 0` for strict zero-failure gating. PASS is not a performance SLO claim for every workload.\n\n");
    md.push_str("## Output files\n\n- `report.json`: full structured report.\n- `metrics.csv`: system resource and background health time series.\n- `report.md`: this report.\n- `trace.log`: tracing log for this run (controlled by `--log-level` / `ACTS_LOG`, default `info`; use `acts=debug,acts_example_stress=debug` for bottleneck analysis and `acts=trace` to see every operation's worker lane. Each `scheduler lane executing operation` line is one plan transition; count them per pid to measure transitions/workflow).\n");
    fs::write(path, md)
}

/// Install the same `ACTS_LOG`/`EnvFilter` contract acts-server uses: the
/// filter's directives select engine and harness targets, and one stream is
/// mirrored to stdout and `<output>/trace.log` so the run can be analyzed after
/// it finished. The file is opened before the engine starts, so a bad filter or
/// unwritable output directory fails the run immediately.
fn init_tracing(options: &Options) -> Result<PathBuf, Box<dyn std::error::Error>> {
    use tracing_subscriber::{EnvFilter, fmt::writer::MakeWriterExt};

    let path = options.output.join("trace.log");
    let file = std::fs::File::create(&path)?;
    let filter = EnvFilter::try_new(&options.log_level).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid log filter {:?}: {error}", options.log_level),
        )
    })?;
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stdout.and(file))
        .with_ansi(false)
        .init();
    Ok(path)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = Options::parse().unwrap_or_else(|error| {
        eprintln!("error: {error}");
        std::process::exit(2);
    });
    fs::create_dir_all(&options.output)?;
    let trace_path = init_tracing(&options)?;

    let maximum_concurrency = *options
        .concurrency
        .last()
        .expect("validated concurrency list");
    let cache_cap = maximum_concurrency.saturating_mul(2).saturating_add(64);
    let queue_cap = maximum_concurrency.saturating_mul(32).max(1_024);

    println!("Acts million-run stress test");
    println!("  target:       {} workflows", options.count);
    println!("  concurrency:  {:?}", options.concurrency);
    println!("  scheduler:    {} workers", options.scheduler_workers);
    println!("  store writer: {} workers", options.store_workers);
    println!("  store:        {}", options.store);
    println!("  cache cap:    {}", cache_cap);
    println!("  soak:         {} seconds", options.soak_seconds);
    println!("  output:       {}", options.output.display());
    println!("  trace log:    {}", trace_path.display());
    println!();
    info!(
        count = options.count,
        concurrency = ?options.concurrency,
        warmup = options.warmup,
        soak_seconds = options.soak_seconds,
        sample_ms = options.sample_ms,
        timeout_seconds = options.timeout_seconds,
        scheduler_workers = options.scheduler_workers,
        store_workers = options.store_workers,
        store = %options.store,
        cache_cap,
        queue_cap,
        output = %options.output.display(),
        log_level = %options.log_level,
        "stress run starting"
    );
    let store: Arc<dyn KvStore> = if options.store == "memory" {
        Arc::new(MemoryStore::new())
    } else {
        Arc::new(SledStore::open("./data")?)
    };
    let engine = Engine::builder()
        .set_store(store)
        .cache_size(cache_cap as i64)
        .scheduler_workers(options.scheduler_workers)
        .scheduler_queue_cap(queue_cap)
        .store_writer_workers(options.store_workers)
        .store_writer_queue_cap(queue_cap)
        .start()
        .await?;
    info!(store = %options.store, "engine started");
    let principal = Principal::unrestricted();
    let executor = engine.executor(&principal);
    let workflow = Workflow::from_yml(STRESS_WORKFLOW)?;
    executor.model().deploy(&workflow, None).await?;
    info!(workflow = %workflow.id, "workflow deployed");

    let origin = Instant::now();
    let phase_label = Arc::new(RwLock::new(("baseline".to_string(), 0usize)));
    let metric_samples = Arc::new(Mutex::new(Vec::<MetricPoint>::new()));
    let metric_errors = Arc::new(AtomicU64::new(0));
    if let Ok(status) = collect_status(&engine, &principal).await {
        metric_samples.lock().push(MetricPoint::new(
            origin.elapsed().as_secs_f64(),
            "baseline".into(),
            0,
            status,
        ));
    } else {
        metric_errors.fetch_add(1, Ordering::Relaxed);
    }
    let monitor_stop = CancellationToken::new();
    let monitor = tokio::spawn(
        MetricsSampler {
            engine: engine.clone(),
            principal: principal.clone(),
            stop: monitor_stop.clone(),
            interval: Duration::from_millis(options.sample_ms),
            origin,
            phase: phase_label.clone(),
            samples: metric_samples.clone(),
            collection_errors: metric_errors.clone(),
        }
        .run(),
    );

    let timeout_seconds = options.timeout_seconds;
    let mut phases = Vec::new();
    let warmup_count = options.warmup.min(options.count);
    *phase_label.write() = ("warmup".into(), options.concurrency[0]);
    println!(
        "[warmup] {} workflows at concurrency {}",
        warmup_count, options.concurrency[0]
    );
    let warmup = run_phase(
        &executor,
        &workflow.id,
        "warmup".into(),
        options.concurrency[0],
        Some(warmup_count),
        None,
        timeout_seconds,
    )
    .await;
    println!(
        "[warmup] completed {}/{} in {:.2}s ({:.1}/s), errors {}",
        warmup.completed,
        warmup.attempted,
        warmup.elapsed_seconds,
        warmup.workflows_per_second,
        warmup.errored + warmup.terminated,
    );
    phases.push(warmup);

    let stages = options.concurrency.len() as u64;
    let per_stage = options.count / stages;
    let remainder = options.count % stages;
    for (index, concurrency) in options.concurrency.iter().copied().enumerate() {
        let target = per_stage + if (index as u64) < remainder { 1 } else { 0 };
        let name = format!("load-{}/{}-c{}", index + 1, stages, concurrency);
        *phase_label.write() = (name.clone(), concurrency);
        println!("[{name}] submitting {target} workflows...");
        let phase = run_phase(
            &executor,
            &workflow.id,
            name.clone(),
            concurrency,
            Some(target),
            None,
            timeout_seconds,
        )
        .await;
        println!(
            "[{name}] attempted {} completed {} errors {} throughput {:.1}/s p99 {:.2}ms",
            phase.attempted,
            phase.completed,
            phase.errored + phase.terminated,
            phase.workflows_per_second,
            phase
                .end_to_end_latency
                .as_ref()
                .map(|s| s.p99_ms)
                .unwrap_or(0.0),
        );
        phases.push(phase);
    }

    if options.soak_seconds > 0 {
        let concurrency = maximum_concurrency;
        let name = format!("steady-soak-{concurrency}c-{}s", options.soak_seconds);
        *phase_label.write() = (name.clone(), concurrency);
        println!(
            "[{name}] holding concurrency for {}s...",
            options.soak_seconds
        );
        let phase = run_phase(
            &executor,
            &workflow.id,
            name.clone(),
            concurrency,
            None,
            Some(Duration::from_secs(options.soak_seconds)),
            timeout_seconds,
        )
        .await;
        println!(
            "[{name}] completed {} errors {} throughput {:.1}/s",
            phase.completed,
            phase.errored + phase.terminated,
            phase.workflows_per_second,
        );
        phases.push(phase);
    }

    let smoke_count = 100u64.min(options.count);
    let smoke_concurrency = options.concurrency[0].clamp(1, 16);
    *phase_label.write() = ("post-stress-smoke".into(), smoke_concurrency);
    println!(
        "[post-stress-smoke] {} workflows at concurrency {}",
        smoke_count, smoke_concurrency
    );
    let smoke = run_phase(
        &executor,
        &workflow.id,
        "post-stress-smoke".into(),
        smoke_concurrency,
        Some(smoke_count),
        None,
        timeout_seconds,
    )
    .await;
    println!(
        "[post-stress-smoke] completed {}/{} in {:.2}s ({:.1}/s), errors {}",
        smoke.completed,
        smoke.attempted,
        smoke.elapsed_seconds,
        smoke.workflows_per_second,
        smoke.errored + smoke.terminated,
    );
    let smoke_incomplete = smoke.attempted < smoke_count;
    phases.push(smoke);

    monitor_stop.cancel();
    let _ = monitor.await;
    if let Ok(status) = collect_status(&engine, &principal).await {
        metric_samples.lock().push(MetricPoint::new(
            origin.elapsed().as_secs_f64(),
            "final".into(),
            0,
            status,
        ));
    } else {
        metric_errors.fetch_add(1, Ordering::Relaxed);
    }

    // Warmup and post-stress smoke are reported but excluded from the million
    // measured runs and the headline rate.
    let load_phases = phases
        .iter()
        .filter(|p| p.name.starts_with("load-"))
        .collect::<Vec<_>>();
    let tested_phases = phases
        .iter()
        .filter(|p| {
            p.name.starts_with("load-")
                || p.name.starts_with("steady-soak-")
                || p.name.starts_with("post-stress-smoke")
        })
        .collect::<Vec<_>>();
    // Headline million-run totals cover exactly --count; soak results remain
    // visible as a separate phase and still participate in the stability gate.
    let total_attempted = load_phases.iter().map(|p| p.attempted).sum::<u64>();
    let total_completed = load_phases.iter().map(|p| p.completed).sum::<u64>();
    let total_errored = load_phases.iter().map(|p| p.errored).sum::<u64>();
    let total_terminated = load_phases.iter().map(|p| p.terminated).sum::<u64>();
    let total_elapsed = load_phases.iter().map(|p| p.elapsed_seconds).sum::<f64>();
    let error_rate = if total_attempted > 0 {
        (total_errored + total_terminated) as f64 * 100.0 / total_attempted as f64
    } else {
        0.0
    };
    let error_gate_failed = tested_phases
        .iter()
        .any(|p| p.error_rate_pct > options.max_error_rate_pct);
    let metrics = metric_samples.lock().clone();
    let (max_rss, min_available, max_proc_cpu, max_sys_cpu, degraded_health) =
        summarize_metrics(&metrics);
    let avg_process_cpu = if metrics.is_empty() {
        0.0
    } else {
        metrics.iter().map(|p| p.cpu.process_percent).sum::<f32>() / metrics.len() as f32
    };
    let avg_system_cpu = if metrics.is_empty() {
        0.0
    } else {
        metrics.iter().map(|p| p.cpu.system_percent).sum::<f32>() / metrics.len() as f32
    };
    let max_storage_used = metrics.iter().filter_map(|p| p.storage.used_bytes).max();
    let max_process_rows = metrics.iter().map(|p| p.processes.total).max().unwrap_or(0);
    let max_active_processes = metrics
        .iter()
        .map(|p| p.processes.active)
        .max()
        .unwrap_or(0);
    let logical_cores = metrics.last().map(|p| p.cpu.logical_cores).unwrap_or(0);
    let has_timeout = tested_phases.iter().any(|p| p.timed_out);
    let metrics_collection_errors = metric_errors.load(Ordering::Relaxed);
    let status_ok = !metrics.is_empty()
        && metrics_collection_errors == 0
        && metrics.iter().all(|p| p.health.state == "healthy");
    let result_status = if has_timeout || smoke_incomplete || !status_ok || error_gate_failed {
        "FAIL"
    } else if total_attempted < options.count {
        "INCOMPLETE"
    } else {
        "PASS"
    };

    let shutdown_started = Instant::now();
    engine.close().await;
    let shutdown_seconds = shutdown_started.elapsed().as_secs_f64();
    info!(shutdown_seconds, "engine closed");

    let report = FinalReport {
        generated_at_unix_seconds: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        status: result_status.into(),
        total_attempted,
        total_completed,
        total_errored,
        total_terminated,
        overall_error_rate_pct: error_rate,
        elapsed_seconds: total_elapsed,
        workflows_per_second: if total_elapsed > 0.0 { total_completed as f64 / total_elapsed } else { 0.0 },
        max_rss_bytes: max_rss,
        min_system_available_bytes: min_available,
        max_process_cpu_percent: max_proc_cpu,
        avg_process_cpu_percent: avg_process_cpu,
        max_system_cpu_percent: max_sys_cpu,
        avg_system_cpu_percent: avg_system_cpu,
        max_storage_used_bytes: max_storage_used,
        max_process_rows,
        max_active_processes,
        logical_cores,
        degraded_health_samples: degraded_health,
        phases,
        metrics,
        metrics_collection_errors,
        config: ConfigReport {
            measured_workflows: options.count,
            concurrency_stages: options.concurrency.clone(),
            warmup_workflows: warmup_count,
            soak_seconds: options.soak_seconds,
            scheduler_workers: options.scheduler_workers,
            store_writer_workers: options.store_workers,
            cache_cap,
            scheduler_queue_cap: queue_cap,
            store_writer_queue_cap: queue_cap,
            sample_interval_ms: options.sample_ms,
            workflow_timeout_seconds: options.timeout_seconds,
            engine_version: env!("CARGO_PKG_VERSION").into(),
            store_backend: if options.store == "memory" {
                "memory".into()
            } else {
                "sled (./data)".into()
            },
            max_error_rate_pct: options.max_error_rate_pct,
            workload: "workflow: init + variable while-loop + expression evaluation + two-way branch + finalize".into(),
            host_os: std::env::consts::OS.into(),
            host_arch: std::env::consts::ARCH.into(),
            logical_cores,
            note: "Backend selected by --store; closed-loop load; sampled peaks may understate instantaneous peaks. Run a release build on the target machine.".into(),
        },
        shutdown_seconds,
    };

    info!(
        status = %report.status,
        attempted = report.total_attempted,
        completed = report.total_completed,
        errored = report.total_errored,
        terminated = report.total_terminated,
        error_rate_pct = report.overall_error_rate_pct,
        elapsed_seconds = report.elapsed_seconds,
        workflows_per_second = report.workflows_per_second,
        max_rss_bytes = report.max_rss_bytes,
        max_process_cpu_percent = report.max_process_cpu_percent,
        max_system_cpu_percent = report.max_system_cpu_percent,
        "stress run finished"
    );
    let json_path = options.output.join("report.json");
    let csv_path = options.output.join("metrics.csv");
    let markdown_path = options.output.join("report.md");
    fs::write(&json_path, serde_json::to_vec_pretty(&report)?)?;
    write_metrics_csv(&csv_path, &report.metrics)?;
    write_markdown(&markdown_path, &report)?;

    println!();
    println!("============================================================");
    println!("RESULT: {}", report.status);
    println!(
        "measured completed {}/{} | errors {} | error rate {:.4}% | {:.1} workflows/s",
        report.total_completed,
        options.count,
        report.total_errored + report.total_terminated,
        report.overall_error_rate_pct,
        report.workflows_per_second,
    );
    println!(
        "peak RSS {:.2} GiB | min available RAM {:.2} GiB | health degradation samples {}",
        report.max_rss_bytes as f64 / 1_073_741_824.0,
        report.min_system_available_bytes as f64 / 1_073_741_824.0,
        report.degraded_health_samples,
    );
    println!("reports: {}", options.output.display());
    println!("  - {}", json_path.display());
    println!("  - {}", csv_path.display());
    println!("  - {}", markdown_path.display());
    println!("  - {}", trace_path.display());

    if report.status == "FAIL" {
        std::process::exit(2);
    }
    if report.status == "INCOMPLETE" {
        std::process::exit(3);
    }
    Ok(())
}
