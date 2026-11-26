# Acts million-run stress report

- Generated at (Unix seconds): 1795691280
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 2260.45s
- Measured throughput: 44.24 workflows/s
- Peak RSS: 1.47 GiB
- Min system available memory: 13.21 GiB
- Process CPU: peak 355.6% / sample mean 318.0%
- System CPU: peak 85.3% / sample mean 78.7%
- Max storage used: 1.43 GiB
- Peak process rows: 795 (active 16)
- Process rows at the end (after the cleanup wait, when configured): 0
- Residual aggregate documents: 0
- Residual index documents: 0
- Logical CPUs: 4
- Degraded health samples: 0
- Metric collection failures: 0
- Engine shutdown: 0.000s

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
| warmup | 16 | 100 | 100 | 0 | 0 | 51.40 | 0.0000% | 1.95 |

**warmup start API latency (ms)**: p50 4.868 / p90 13.261 / p95 19.386 / p99 19.747 / p99.9 19.785 / max 19.785 / mean 6.589 / stddev 4.474
**warmup end-to-end latency (ms)**: p50 296.455 / p90 472.061 / p95 530.394 / p99 693.731 / p99.9 739.031 / max 739.031 / mean 296.854 / stddev 146.720

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 44.24 | 0.0000% | 2260.45 |

**load-1/1-c16 start API latency (ms)**: p50 5.297 / p90 7.177 / p95 7.821 / p99 9.224 / p99.9 15.461 / max 815.247 / mean 5.504 / stddev 3.646
**load-1/1-c16 end-to-end latency (ms)**: p50 302.143 / p90 728.463 / p95 864.006 / p99 1086.943 / p99.9 1263.425 / max 1916.339 / mean 361.649 / stddev 252.537

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 47.41 | 0.0000% | 2.11 |

**post-stress-smoke start API latency (ms)**: p50 4.874 / p90 14.681 / p95 16.316 / p99 17.662 / p99.9 19.321 / max 19.321 / mean 6.307 / stddev 4.086
**post-stress-smoke end-to-end latency (ms)**: p50 288.057 / p90 702.817 / p95 789.181 / p99 871.419 / p99.9 950.844 / max 950.844 / mean 317.533 / stddev 233.072

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
