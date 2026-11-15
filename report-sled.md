# Acts million-run stress report

- Generated at (Unix seconds): 1794712988
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 1731.57s
- Measured throughput: 57.75 workflows/s
- Peak RSS: 0.72 GiB
- Min system available memory: 13.67 GiB
- Process CPU: peak 335.1% / sample mean 306.0%
- System CPU: peak 81.6% / sample mean 75.0%
- Max storage used: 1.11 GiB
- Peak process rows: 975 (active 16)
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
| warmup | 16 | 100 | 100 | 0 | 0 | 67.00 | 0.0000% | 1.49 |

**warmup start API latency (ms)**: p50 5.415 / p90 11.425 / p95 13.202 / p99 14.229 / p99.9 14.541 / max 14.541 / mean 6.503 / stddev 2.766
**warmup end-to-end latency (ms)**: p50 190.294 / p90 434.613 / p95 514.184 / p99 569.215 / p99.9 613.356 / max 613.356 / mean 226.202 / stddev 146.532

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 57.75 | 0.0000% | 1731.57 |

**load-1/1-c16 start API latency (ms)**: p50 5.962 / p90 8.145 / p95 8.834 / p99 10.275 / p99.9 12.305 / max 374.256 / mean 6.102 / stddev 1.936
**load-1/1-c16 end-to-end latency (ms)**: p50 229.829 / p90 558.689 / p95 665.326 / p99 851.592 / p99.9 1034.382 / max 1204.281 / mean 277.037 / stddev 195.081

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 63.44 | 0.0000% | 1.58 |

**post-stress-smoke start API latency (ms)**: p50 5.337 / p90 12.409 / p95 14.477 / p99 16.044 / p99.9 16.474 / max 16.474 / mean 6.436 / stddev 3.347
**post-stress-smoke end-to-end latency (ms)**: p50 211.128 / p90 405.678 / p95 506.753 / p99 578.617 / p99.9 597.325 / max 597.325 / mean 236.016 / stddev 142.357

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
