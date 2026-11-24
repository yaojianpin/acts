# Acts million-run stress report

- Generated at (Unix seconds): 1795531440
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 3402.38s
- Measured throughput: 29.39 workflows/s
- Peak RSS: 2.67 GiB
- Min system available memory: 11.97 GiB
- Process CPU: peak 384.4% / sample mean 325.0%
- System CPU: peak 93.5% / sample mean 81.6%
- Max storage used: 0.28 GiB
- Peak process rows: 829 (active 16)
- Process rows at the end (after the cleanup wait, when configured): 0
- Residual aggregate documents: 0
- Residual index documents: 0
- Logical CPUs: 4
- Degraded health samples: 0
- Metric collection failures: 0
- Engine shutdown: 0.263s

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
| warmup | 16 | 100 | 100 | 0 | 0 | 48.03 | 0.0000% | 2.08 |

**warmup start API latency (ms)**: p50 4.338 / p90 13.431 / p95 14.711 / p99 16.149 / p99.9 16.506 / max 16.506 / mean 6.027 / stddev 3.727
**warmup end-to-end latency (ms)**: p50 303.724 / p90 567.518 / p95 640.071 / p99 669.942 / p99.9 753.400 / max 753.400 / mean 316.662 / stddev 176.512

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 29.39 | 0.0000% | 3402.38 |

**load-1/1-c16 start API latency (ms)**: p50 6.331 / p90 13.552 / p95 15.717 / p99 19.729 / p99.9 26.030 / max 814.465 / mean 7.858 / stddev 4.635
**load-1/1-c16 end-to-end latency (ms)**: p50 439.545 / p90 1119.079 / p95 1358.659 / p99 1795.299 / p99.9 2227.675 / max 2717.871 / mean 544.339 / stddev 403.485

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 32.42 | 0.0000% | 3.08 |

**post-stress-smoke start API latency (ms)**: p50 5.973 / p90 20.404 / p95 22.921 / p99 23.785 / p99.9 23.911 / max 23.911 / mean 9.684 / stddev 6.346
**post-stress-smoke end-to-end latency (ms)**: p50 410.431 / p90 801.359 / p95 1067.267 / p99 1275.322 / p99.9 1388.316 / max 1388.316 / mean 459.085 / stddev 275.069

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
