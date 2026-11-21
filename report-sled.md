# Acts million-run stress report

- Generated at (Unix seconds): 1795251803
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 3326.39s
- Measured throughput: 30.06 workflows/s
- Peak RSS: 1.12 GiB
- Min system available memory: 13.47 GiB
- Process CPU: peak 350.7% / sample mean 313.0%
- System CPU: peak 86.7% / sample mean 79.4%
- Max storage used: 1.05 GiB
- Peak process rows: 557 (active 16)
- Process rows at the end (after the cleanup wait, when configured): 0
- Residual aggregate documents: 0
- Residual index documents: 0
- Logical CPUs: 4
- Degraded health samples: 0
- Metric collection failures: 0
- Engine shutdown: 0.001s

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
- Store backend: sled (target/stress-results/sled-baseline-store, 4 shards, 50ms flush)
- Error-rate gate: 0.000%

Backend selected by --store; connection URLs are not copied into this report; closed-loop load; sampled peaks may understate instantaneous peaks. Run a release build on the target machine.

## Per-phase results

| Phase | Concurrency | Attempted | Completed | Errored | Terminated | Throughput/s | Error rate | Elapsed/s |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| warmup | 16 | 100 | 100 | 0 | 0 | 32.27 | 0.0000% | 3.10 |

**warmup start API latency (ms)**: p50 7.272 / p90 23.369 / p95 28.109 / p99 32.541 / p99.9 35.387 / max 35.387 / mean 10.034 / stddev 7.243
**warmup end-to-end latency (ms)**: p50 368.353 / p90 1000.419 / p95 1043.449 / p99 1184.148 / p99.9 1227.349 / max 1227.349 / mean 462.891 / stddev 323.474

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 30.06 | 0.0000% | 3326.39 |

**load-1/1-c16 start API latency (ms)**: p50 7.791 / p90 10.491 / p95 11.410 / p99 13.473 / p99.9 17.364 / max 31.772 / mean 8.019 / stddev 1.909
**load-1/1-c16 end-to-end latency (ms)**: p50 447.602 / p90 1065.724 / p95 1261.809 / p99 1583.464 / p99.9 1874.814 / max 2305.309 / mean 532.195 / stddev 366.925

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 27.36 | 0.0000% | 3.65 |

**post-stress-smoke start API latency (ms)**: p50 8.867 / p90 21.975 / p95 24.443 / p99 28.859 / p99.9 32.411 / max 32.411 / mean 11.452 / stddev 6.104
**post-stress-smoke end-to-end latency (ms)**: p50 529.226 / p90 980.419 / p95 1085.238 / p99 1228.488 / p99.9 1284.337 / max 1284.337 / mean 555.323 / stddev 301.628

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
