# Acts million-run stress report

- Generated at (Unix seconds): 1794517646
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 2357.25s
- Measured throughput: 42.42 workflows/s
- Peak RSS: 0.76 GiB
- Min system available memory: 13.88 GiB
- Process CPU: peak 350.7% / sample mean 314.8%
- System CPU: peak 88.9% / sample mean 80.0%
- Max storage used: 0.66 GiB
- Peak process rows: 690 (active 16)
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
| warmup | 16 | 100 | 100 | 0 | 0 | 42.02 | 0.0000% | 2.38 |

**warmup start API latency (ms)**: p50 10.580 / p90 20.762 / p95 23.686 / p99 26.479 / p99.9 28.552 / max 28.552 / mean 12.039 / stddev 4.959
**warmup end-to-end latency (ms)**: p50 358.575 / p90 562.899 / p95 607.829 / p99 718.285 / p99.9 727.483 / max 727.483 / mean 361.787 / stddev 158.486

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 42.42 | 0.0000% | 2357.25 |

**load-1/1-c16 start API latency (ms)**: p50 8.425 / p90 11.410 / p95 12.393 / p99 14.517 / p99.9 18.111 / max 407.703 / mean 8.636 / stddev 2.585
**load-1/1-c16 end-to-end latency (ms)**: p50 317.318 / p90 755.206 / p95 891.879 / p99 1127.001 / p99.9 1342.115 / max 1490.365 / mean 377.128 / stddev 259.614

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 35.94 | 0.0000% | 2.78 |

**post-stress-smoke start API latency (ms)**: p50 9.432 / p90 21.583 / p95 26.777 / p99 29.356 / p99.9 37.742 / max 37.742 / mean 11.415 / stddev 6.432
**post-stress-smoke end-to-end latency (ms)**: p50 308.951 / p90 936.089 / p95 1133.399 / p99 1235.527 / p99.9 1304.185 / max 1304.185 / mean 429.197 / stddev 329.288

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
