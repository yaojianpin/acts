//! Completion-latency microbenchmark for the in-memory engine path.
//!
//! `cargo run --release -p acts --example prof -- [OPTIONS]`
//!
//! Pre-creates `--iters` processes parked on one human act, then completes them
//! one by one and reports the per-completion latency distribution. The
//! measurement starts after `--warmup` completions, and the arming channel is
//! closed before the warmup so the completion path does not pay message fan-out
//! that `benches/workflow.rs` (no channel registered) does not pay either.
//!
//! This is a microbenchmark, not a load test: it measures the deterministic
//! in-memory engine path, so its numbers are comparable between runs in a way a
//! shared-runner end-to-end test is not (see `.github/workflows/prof.yml`).
use acts::{Engine, MessageState, Vars, Workflow};
use acts_store::{RocksdbStore, Shard, ShardConfig, ShardedStore, SledStore};
use parking_lot::Mutex;
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

/// The arming subscription waits for one message per parked process; a store
/// or scheduler that never delivers them is a broken run, not a slow one.
const ARM_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Store {
    Memory,
    Sled,
    Rocksdb,
}

impl Store {
    fn as_str(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Sled => "sled",
            Self::Rocksdb => "rocksdb",
        }
    }
}

#[derive(Debug, Clone)]
struct Options {
    iters: u32,
    warmup: u32,
    store: Store,
    store_shards: usize,
    store_url: Option<PathBuf>,
    json: Option<PathBuf>,
    markdown: Option<PathBuf>,
    help: bool,
}

impl Default for Options {
    fn default() -> Self {
        // The env fallback keeps the historical invocation working; the CLI
        // flag wins when both are given.
        let iters = std::env::var("PROF_ITERS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(2000u32)
            .max(2);
        Self {
            iters,
            warmup: 1,
            store: Store::Memory,
            store_shards: 1,
            store_url: None,
            json: None,
            markdown: None,
            help: false,
        }
    }
}

fn arg(args: &mut impl Iterator<Item = String>, name: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("missing value after {name}"))
}

fn parse_args() -> Result<Options, String> {
    let mut options = Options::default();
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--iters" => {
                options.iters = arg(&mut args, "--iters")?
                    .parse()
                    .map_err(|error| format!("invalid --iters: {error}"))?;
            }
            "--warmup" => {
                options.warmup = arg(&mut args, "--warmup")?
                    .parse()
                    .map_err(|error| format!("invalid --warmup: {error}"))?;
            }
            "--store" => {
                options.store = match arg(&mut args, "--store")?.as_str() {
                    "memory" => Store::Memory,
                    "sled" => Store::Sled,
                    "rocksdb" => Store::Rocksdb,
                    other => {
                        return Err(format!(
                            "--store must be memory, sled or rocksdb, got {other:?}"
                        ));
                    }
                };
            }
            "--store-shards" => {
                options.store_shards = arg(&mut args, "--store-shards")?
                    .parse()
                    .map_err(|error| format!("invalid --store-shards: {error}"))?;
            }
            "--store-url" => {
                options.store_url = Some(PathBuf::from(arg(&mut args, "--store-url")?))
            }
            "--json" => options.json = Some(PathBuf::from(arg(&mut args, "--json")?)),
            "--markdown" => options.markdown = Some(PathBuf::from(arg(&mut args, "--markdown")?)),
            "--help" | "-h" => options.help = true,
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    if options.iters < 2 {
        return Err("--iters must be at least 2".into());
    }
    if options.warmup >= options.iters {
        return Err("--warmup must be smaller than --iters".into());
    }
    if options.store == Store::Memory && options.store_url.is_some() {
        return Err("--store-url requires --store sled or rocksdb".into());
    }
    if options.store_shards == 0 || options.store_shards > 1_024 {
        return Err("--store-shards must be between 1 and 1024".into());
    }
    if options.store == Store::Memory && options.store_shards != 1 {
        return Err("--store-shards > 1 requires --store sled or rocksdb".into());
    }
    Ok(options)
}

fn usage() {
    println!(
        "examples/prof — completion-latency microbenchmark\n\n\
         USAGE:\n  cargo run --release -p acts --example prof -- [OPTIONS]\n\n\
         OPTIONS:\n\
           --iters N       parked processes to arm and complete (default 2000, at least 2)\n\
           --warmup N      completions before the measured window (default 1)\n\
           --store KIND    memory (default), sled or rocksdb\n\
           --store-shards N physical shards for sled/rocksdb (default 1)\n\
           --store-url P   sled/rocksdb base directory (default target/prof-<store>;\n\
                           each shard uses <base>/shard-<i>)\n\
           --json PATH     write the JSON report\n\
           --markdown PATH write the Markdown report\n\
           -h, --help      show this help\n\n\
         The environment variable PROF_ITERS is the fallback for --iters."
    );
}

/// The `prof` report schema. It is a small, stable shape on purpose: CI reads
/// `status`, `avg_us`, `p99_us` and `completions_per_second` for the badges,
/// and a reader can diff the JSON between two commits without a parser.
#[derive(Debug, Serialize)]
struct Report {
    status: &'static str,
    store: &'static str,
    shards: usize,
    iters: u32,
    warmup: u32,
    samples: usize,
    errors: usize,
    total_ms: f64,
    avg_us: f64,
    p50_us: f64,
    p95_us: f64,
    p99_us: f64,
    max_us: f64,
    completions_per_second: f64,
    engine_version: &'static str,
    profile: &'static str,
}

fn micros(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e6
}

fn pick(samples: &[Duration], percentile: f64) -> Duration {
    // Nearest-rank, clamped to the sample window. `samples` is sorted and
    // non-empty by the time this is called.
    let index = ((percentile / 100.0) * samples.len() as f64).ceil() as usize;
    samples[index.clamp(1, samples.len()) - 1]
}

fn write_report(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)
}

fn markdown(report: &Report) -> String {
    format!(
        "# `examples/prof` report\n\n\
         | metric | value |\n\
         | --- | --- |\n\
         | Result | {} |\n\
         | Store | {} ({} shards) |\n\
         | Samples | {} (iters {}, warmup {}) |\n\
         | Errors | {} |\n\
         | Total measured time | {:.3} ms |\n\
         | Average completion | {:.3} µs |\n\
         | p50 / p95 / p99 | {:.3} / {:.3} / {:.3} µs |\n\
         | Max | {:.3} µs |\n\
         | Throughput | {:.1} completions/s |\n\
         | Profile | {} |\n\
         | Engine version | {} |\n\n\
         Completion latency is trend data, not a fixed threshold: a shared CI\n\
         runner's timings move by double-digit percents between runs. The gate\n\
         is `Result`, which fails on any completion error.\n",
        report.status,
        report.store,
        report.shards,
        report.samples,
        report.iters,
        report.warmup,
        report.errors,
        report.total_ms,
        report.avg_us,
        report.p50_us,
        report.p95_us,
        report.p99_us,
        report.max_us,
        report.completions_per_second,
        report.profile,
        report.engine_version,
    )
}

#[tokio::main]
async fn main() {
    let options = match parse_args() {
        Ok(options) => options,
        Err(error) => {
            eprintln!("error: {error}\n");
            usage();
            std::process::exit(2);
        }
    };
    if options.help {
        usage();
        return;
    }

    let text = r#"
id: act_test
ver: 0.1.0
steps:
  - id: step1
    uses: acts.core.irq
    params:
      key: act1
"#;
    let workflow = Workflow::from_yml(text).unwrap();
    let mut config = acts::Config::default();
    config.data.cache_cap = Some(100_000);
    let mut builder = Engine::builder().set_config(&config);
    match options.store {
        Store::Memory => {}
        Store::Sled | Store::Rocksdb => {
            // The directory is the measured medium. A caller that wants to
            // keep it for inspection passes --store-url; CI gives it a path
            // under the run's output directory. Sharded runs put each physical
            // database under <base>/shard-<i>.
            let default = format!("target/prof-{}", options.store.as_str());
            let base = options
                .store_url
                .clone()
                .unwrap_or_else(|| PathBuf::from(default));
            let shard_config = || ShardConfig {
                max_in_flight_writes: 16,
                ..ShardConfig::default()
            };
            if options.store_shards > 1 {
                let mut shards = Vec::with_capacity(options.store_shards);
                for index in 0..options.store_shards {
                    let path = base.join(format!("shard-{index}"));
                    let id = path.to_string_lossy().into_owned();
                    let store: Arc<dyn acts::KvStore> = match options.store {
                        Store::Sled => Arc::new(
                            SledStore::open(&path.to_string_lossy())
                                .expect("failed to open sled shard"),
                        ),
                        Store::Rocksdb => Arc::new(
                            RocksdbStore::open(&path.to_string_lossy())
                                .expect("failed to open rocksdb shard"),
                        ),
                        Store::Memory => unreachable!("memory is not sharded"),
                    };
                    shards.push(Shard::new(id, store));
                }
                let store = ShardedStore::with_config(shards, shard_config())
                    .expect("failed to build sharded store");
                builder = builder.set_store(Arc::new(store));
            } else {
                match options.store {
                    Store::Sled => {
                        let store = SledStore::open(&base.to_string_lossy())
                            .expect("failed to open sled store");
                        builder = builder.set_store(Arc::new(store));
                    }
                    Store::Rocksdb => {
                        let store = RocksdbStore::open(&base.to_string_lossy())
                            .expect("failed to open rocksdb store");
                        builder = builder.set_store(Arc::new(store));
                    }
                    Store::Memory => unreachable!("memory is not durable"),
                }
            }
        }
    }
    let engine = builder.start().await.expect("failed to start engine");
    engine
        .executor(&acts::Principal::unrestricted())
        .model()
        .deploy(&workflow, None)
        .await
        .unwrap();

    let (closer, receiver) = engine.signal(()).double();
    let tasks = Arc::new(Mutex::new(Vec::new()));
    let observer = tasks.clone();
    let channel = engine.channel();
    let iters = options.iters as usize;
    channel.on_message(move |event| {
        let observer = observer.clone();
        let closer = closer.clone();
        async move {
            if event.is_params_key("act1") && event.is_state(MessageState::Created) {
                let mut armed = observer.lock();
                armed.push((event.pid.clone(), event.tid.clone()));
                if armed.len() >= iters {
                    closer.close();
                }
            }
        }
    });

    for _ in 0..options.iters {
        engine
            .executor(&acts::Principal::unrestricted())
            .proc()
            .start(&workflow.id, Vars::new())
            .await
            .unwrap();
    }
    if tokio::time::timeout(ARM_TIMEOUT, receiver.recv())
        .await
        .is_err()
    {
        eprintln!(
            "error: timed out after {ARM_TIMEOUT:?} waiting for {iters} parked processes to arm"
        );
        std::process::exit(3);
    }
    // Close the arming subscription before measuring: an open channel receives
    // every completion's events, which charges the measurement for message
    // fan-out that `benches/workflow.rs` (no channel registered) does not pay.
    channel.close();
    let tasks = {
        let guard = tasks.lock();
        println!("pre-created {} tasks", guard.len());
        guard.clone()
    };
    if tasks.len() < iters {
        eprintln!(
            "error: armed {} tasks, expected {iters}; refusing to report a short sample",
            tasks.len()
        );
        std::process::exit(3);
    }

    // The executor is bound once: constructing it per operation would put the
    // executor's own setup inside the sample, which the criterion benchmark
    // (and a real caller) does not pay per completion.
    let principal = acts::Principal::unrestricted();
    let executor = engine.executor(&principal);
    let warmup = options.warmup as usize;
    let mut errors = 0usize;
    for (pid, tid) in tasks.iter().take(warmup) {
        if executor
            .act()
            .complete(pid, tid, Vars::new())
            .await
            .is_err()
        {
            errors += 1;
        }
    }

    let start = Instant::now();
    let mut samples = Vec::with_capacity(tasks.len() - warmup);
    for (pid, tid) in tasks.iter().skip(warmup) {
        let operation = Instant::now();
        match executor.act().complete(pid, tid, Vars::new()).await {
            Ok(_) => samples.push(operation.elapsed()),
            Err(_) => errors += 1,
        }
    }
    let total = start.elapsed();
    drop(executor);
    engine.close().await;

    samples.sort_unstable();
    let total_seconds = total.as_secs_f64();
    let report = Report {
        status: if errors == 0 && !samples.is_empty() {
            "PASS"
        } else {
            "FAIL"
        },
        store: options.store.as_str(),
        shards: options.store_shards,
        iters: options.iters,
        warmup: options.warmup,
        samples: samples.len(),
        errors,
        total_ms: total_seconds * 1e3,
        avg_us: if samples.is_empty() {
            0.0
        } else {
            total_seconds * 1e6 / samples.len() as f64
        },
        p50_us: if samples.is_empty() {
            0.0
        } else {
            micros(pick(&samples, 50.0))
        },
        p95_us: if samples.is_empty() {
            0.0
        } else {
            micros(pick(&samples, 95.0))
        },
        p99_us: if samples.is_empty() {
            0.0
        } else {
            micros(pick(&samples, 99.0))
        },
        max_us: samples.last().map_or(0.0, |duration| micros(*duration)),
        completions_per_second: if samples.is_empty() || total_seconds == 0.0 {
            0.0
        } else {
            samples.len() as f64 / total_seconds
        },
        engine_version: acts::VERSION,
        profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
    };

    println!(
        "complete x{}: {:.3} ms  avg {:.3} µs  p50 {:.3} µs  p95 {:.3} µs  p99 {:.3} µs  max {:.3} µs  {:.1}/s  errors {}  {} ({})",
        report.samples,
        report.total_ms,
        report.avg_us,
        report.p50_us,
        report.p95_us,
        report.p99_us,
        report.max_us,
        report.completions_per_second,
        report.errors,
        report.status,
        report.store,
    );

    if let Some(path) = &options.json {
        let json = serde_json::to_string_pretty(&report).unwrap();
        if let Err(error) = write_report(path, &json) {
            eprintln!("error: cannot write {}: {error}", path.display());
            std::process::exit(4);
        }
    }
    if let Some(path) = &options.markdown
        && let Err(error) = write_report(path, &markdown(&report))
    {
        eprintln!("error: cannot write {}: {error}", path.display());
        std::process::exit(4);
    }
    if report.status != "PASS" {
        std::process::exit(1);
    }
}
