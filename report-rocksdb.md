# Acts million-run stress report

- Generated at (Unix seconds): 1793841060
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 3684.23s
- Measured throughput: 27.14 workflows/s
- Peak RSS: 6.42 GiB
- Min system available memory: 8.26 GiB
- Process CPU: peak 384.4% / sample mean 341.6%
- System CPU: peak 93.8% / sample mean 84.3%
- Max storage used: 0.78 GiB
- Peak process rows: 862 (active 16)
- Process rows at the end (after the cleanup wait, when configured): 0
- Residual aggregate documents: 0
- Residual index documents: 0
- Logical CPUs: 4
- Degraded health samples: 0
- Metric collection failures: 0
- Engine shutdown: 0.550s

## Test configuration

- Workload: workflow: init + variable while-loop + expression evaluation + two-way branch + finalize
- Host: linux / x86_64 (4 logical CPUs)
- Concurrency stages: [16]
- Warmup workflows: 100
- Steady-state soak: 0s
- Cleanup drain wait: 300s
- Scheduler workers: 4
- Store shards: 16
- Cache cap: 96
- Scheduler queue cap: 1024
- Store writer queue cap: 1024
- Metric sample interval: 1000ms (each status sample also waits for sysinfo's CPU delta window)
- Per-workflow wait timeout: 120s
- Engine version: 0.1.0
- Store backend: rocksdb (target/stress-results/rocksdb-baseline-store, 16 shards)
- Error-rate gate: 0.000%

Backend selected by --store; connection URLs are not copied into this report; closed-loop load; sampled peaks may understate instantaneous peaks. Run a release build on the target machine.

## Per-phase results

| Phase | Concurrency | Attempted | Completed | Errored | Terminated | Throughput/s | Error rate | Elapsed/s |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| warmup | 16 | 100 | 100 | 0 | 0 | 58.75 | 0.0000% | 1.70 |

**warmup start API latency (ms)**: p50 8.725 / p90 16.162 / p95 19.594 / p99 24.428 / p99.9 24.787 / max 24.787 / mean 10.086 / stddev 4.059
**warmup end-to-end latency (ms)**: p50 196.603 / p90 577.776 / p95 662.529 / p99 727.299 / p99.9 766.531 / max 766.531 / mean 248.910 / stddev 183.442

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 27.14 | 0.0000% | 3684.23 |

**load-1/1-c16 start API latency (ms)**: p50 14.488 / p90 41.434 / p95 45.855 / p99 52.102 / p99.9 61.787 / max 409.276 / mean 21.833 / stddev 12.856
**load-1/1-c16 end-to-end latency (ms)**: p50 423.785 / p90 1312.311 / p95 1715.933 / p99 2453.172 / p99.9 3207.536 / max 3935.144 / mean 589.450 / stddev 524.829

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 44.19 | 0.0000% | 2.26 |

**post-stress-smoke start API latency (ms)**: p50 13.746 / p90 20.863 / p95 22.991 / p99 26.263 / p99.9 26.981 / max 26.981 / mean 14.743 / stddev 3.853
**post-stress-smoke end-to-end latency (ms)**: p50 281.978 / p90 680.131 / p95 805.679 / p99 946.750 / p99.9 987.662 / max 987.662 / mean 344.091 / stddev 220.656

## Metric definitions and limitations

- Each run's `input` varies 0–9, so the while-loop length and both branches are exercised.
- The report keeps status metrics only; it never copies `SystemStatus.config`, so credentials in configuration cannot leak into a report.
- start API latency: from the `proc:start` call until a durable run handle is returned.
- end-to-end latency: from submission until `RunHandle::wait` observes a terminal outcome, including queueing and execution.
- workflows/s: completed runs divided by the phase's wall-clock time; errored and terminated runs do not count.
- Percentiles use nearest-rank; stddev is the sample standard deviation.
- CPU/RSS/storage/process counts/background loop health come from the engine's `sys:status`; each CPU sample waits for sysinfo's minimum CPU delta window, so the actual sampling interval is at least `sample-ms` and the sampler itself adds some overhead. Peaks are sample points, not continuous peak values. Collection failures are counted and fail the PASS gate.
- This is a closed-loop load: each worker submits its next run only after the previous one finished; it is not a fixed-arrival-rate open-loop model.
- The backend is selected by `--store`: `sled` (default, `./data`), `memory`, `sqlite` (file in `--store-url` or `<output>/stress.sqlite`), or `postgres` (URL in `--store-url` or `DATABASE_URL`; never copied into this report). `sled` acknowledges before fsync and flushes in the background every 50ms, so a crash can lose that window (and the measured throughput excludes the final shutdown flush); SQLite/Postgres make each commit durable before it returns; `memory` isolates engine cost from storage cost.
- PASS means the measured target was submitted and every measured/soak/post-stress phase stayed under the error-rate gate, with no wait timeouts, a complete post-stress smoke run, healthy background loops and successful metric collection. A few failures under the gate are still reported. Use `--max-error-rate-pct 0` for strict zero-failure gating. PASS is not a performance SLO claim for every workload.

## Output files

- `report.json`: full structured report.
- `metrics.csv`: system resource and background health time series.
- `report.md`: this report.
- `trace.log`: tracing log for this run (controlled by `--log-level` / `ACTS_LOG`, default `info`; use `acts=debug,acts_example_stress=debug` for bottleneck analysis and `acts=trace` to see every operation's worker lane. Each `scheduler lane executing operation` line is one plan transition; count them per pid to measure transitions/workflow).
