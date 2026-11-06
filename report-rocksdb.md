# Acts million-run stress report

- Generated at (Unix seconds): 1793972379
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 1940.19s
- Measured throughput: 51.54 workflows/s
- Peak RSS: 2.44 GiB
- Min system available memory: 12.22 GiB
- Process CPU: peak 357.9% / sample mean 315.3%
- System CPU: peak 89.6% / sample mean 79.1%
- Max storage used: 0.28 GiB
- Peak process rows: 1299 (active 17)
- Process rows at the end (after the cleanup wait, when configured): 0
- Residual aggregate documents: 0
- Residual index documents: 0
- Logical CPUs: 4
- Degraded health samples: 0
- Metric collection failures: 0
- Engine shutdown: 0.528s

## Test configuration

- Workload: workflow: init + variable while-loop + expression evaluation + two-way branch + finalize
- Host: linux / x86_64 (4 logical CPUs)
- Concurrency stages: [16]
- Warmup workflows: 100
- Steady-state soak: 0s
- Cleanup drain wait: 300s
- Scheduler workers: 4
- Store shards: 4
- Cache cap: 96
- Scheduler queue cap: 1024
- Store writer queue cap: 1024
- Metric sample interval: 1000ms (each status sample also waits for sysinfo's CPU delta window)
- Per-workflow wait timeout: 120s
- Engine version: 0.1.0
- Store backend: rocksdb (target/stress-results/rocksdb-baseline-store, 4 shards)
- Error-rate gate: 0.000%

Backend selected by --store; connection URLs are not copied into this report; closed-loop load; sampled peaks may understate instantaneous peaks. Run a release build on the target machine.

## Per-phase results

| Phase | Concurrency | Attempted | Completed | Errored | Terminated | Throughput/s | Error rate | Elapsed/s |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| warmup | 16 | 100 | 100 | 0 | 0 | 88.17 | 0.0000% | 1.13 |

**warmup start API latency (ms)**: p50 3.615 / p90 7.393 / p95 8.220 / p99 9.347 / p99.9 9.751 / max 9.751 / mean 4.222 / stddev 1.866
**warmup end-to-end latency (ms)**: p50 142.963 / p90 356.242 / p95 390.355 / p99 466.754 / p99.9 477.608 / max 477.608 / mean 168.489 / stddev 117.098

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 51.54 | 0.0000% | 1940.19 |

**load-1/1-c16 start API latency (ms)**: p50 5.732 / p90 10.782 / p95 11.935 / p99 14.842 / p99.9 21.797 / max 160.344 / mean 6.664 / stddev 3.071
**load-1/1-c16 end-to-end latency (ms)**: p50 245.167 / p90 651.029 / p95 802.481 / p99 1080.867 / p99.9 1335.357 / max 1628.468 / mean 310.404 / stddev 240.093

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 48.90 | 0.0000% | 2.05 |

**post-stress-smoke start API latency (ms)**: p50 8.220 / p90 12.929 / p95 14.672 / p99 16.813 / p99.9 17.649 / max 17.649 / mean 8.507 / stddev 3.439
**post-stress-smoke end-to-end latency (ms)**: p50 241.918 / p90 601.054 / p95 685.823 / p99 932.330 / p99.9 990.061 / max 990.061 / mean 303.720 / stddev 204.967

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
