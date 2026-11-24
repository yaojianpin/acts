# Acts million-run stress report

- Generated at (Unix seconds): 1795483438
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 3428.57s
- Measured throughput: 29.17 workflows/s
- Peak RSS: 1.12 GiB
- Min system available memory: 13.44 GiB
- Process CPU: peak 314.7% / sample mean 281.1%
- System CPU: peak 78.5% / sample mean 71.7%
- Max storage used: 0.95 GiB
- Peak process rows: 544 (active 16)
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
| warmup | 16 | 100 | 100 | 0 | 0 | 37.53 | 0.0000% | 2.66 |

**warmup start API latency (ms)**: p50 7.375 / p90 16.549 / p95 18.276 / p99 19.875 / p99.9 20.108 / max 20.108 / mean 8.789 / stddev 4.064
**warmup end-to-end latency (ms)**: p50 334.317 / p90 655.212 / p95 761.302 / p99 997.411 / p99.9 1018.447 / max 1018.447 / mean 401.367 / stddev 205.819

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 29.17 | 0.0000% | 3428.57 |

**load-1/1-c16 start API latency (ms)**: p50 8.039 / p90 10.848 / p95 11.767 / p99 13.721 / p99.9 17.241 / max 787.807 / mean 8.279 / stddev 3.664
**load-1/1-c16 end-to-end latency (ms)**: p50 457.693 / p90 1104.598 / p95 1312.069 / p99 1665.676 / p99.9 1949.694 / max 2306.043 / mean 548.545 / stddev 382.680

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 33.65 | 0.0000% | 2.97 |

**post-stress-smoke start API latency (ms)**: p50 7.275 / p90 21.283 / p95 24.322 / p99 28.831 / p99.9 29.982 / max 29.982 / mean 9.633 / stddev 6.203
**post-stress-smoke end-to-end latency (ms)**: p50 407.819 / p90 843.915 / p95 951.751 / p99 1107.015 / p99.9 1205.268 / max 1205.268 / mean 453.236 / stddev 271.499

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
