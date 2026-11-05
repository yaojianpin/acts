# Acts million-run stress report

- Generated at (Unix seconds): 1793884729
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 2335.75s
- Measured throughput: 42.81 workflows/s
- Peak RSS: 0.85 GiB
- Min system available memory: 13.84 GiB
- Process CPU: peak 351.4% / sample mean 315.7%
- System CPU: peak 89.6% / sample mean 79.7%
- Max storage used: 0.68 GiB
- Peak process rows: 701 (active 16)
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
| warmup | 16 | 100 | 100 | 0 | 0 | 45.82 | 0.0000% | 2.18 |

**warmup start API latency (ms)**: p50 8.239 / p90 24.444 / p95 34.620 / p99 39.205 / p99.9 39.620 / max 39.620 / mean 11.485 / stddev 8.472
**warmup end-to-end latency (ms)**: p50 325.047 / p90 553.711 / p95 610.753 / p99 694.985 / p99.9 768.943 / max 768.943 / mean 331.074 / stddev 168.412

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 42.81 | 0.0000% | 2335.75 |

**load-1/1-c16 start API latency (ms)**: p50 8.293 / p90 11.180 / p95 12.131 / p99 14.207 / p99.9 17.898 / max 401.591 / mean 8.484 / stddev 2.422
**load-1/1-c16 end-to-end latency (ms)**: p50 313.318 / p90 752.962 / p95 890.296 / p99 1124.043 / p99.9 1341.117 / max 1625.910 / mean 373.703 / stddev 260.107

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 46.03 | 0.0000% | 2.17 |

**post-stress-smoke start API latency (ms)**: p50 8.210 / p90 20.569 / p95 22.035 / p99 24.482 / p99.9 25.181 / max 25.181 / mean 10.068 / stddev 5.323
**post-stress-smoke end-to-end latency (ms)**: p50 312.049 / p90 577.864 / p95 602.355 / p99 708.723 / p99.9 818.406 / max 818.406 / mean 330.844 / stddev 164.357

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
