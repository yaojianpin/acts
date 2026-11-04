# Acts million-run stress report

- Generated at (Unix seconds): 1793802470
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 642.42s
- Measured throughput: 155.66 workflows/s
- Peak RSS: 0.46 GiB
- Min system available memory: 14.24 GiB
- Process CPU: peak 375.6% / sample mean 291.7%
- System CPU: peak 94.9% / sample mean 73.9%
- Max storage used: 0.23 GiB
- Peak process rows: 3431 (active 17)
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
- Store shards: 1
- Cache cap: 96
- Scheduler queue cap: 1024
- Store writer queue cap: 1024
- Metric sample interval: 1000ms (each status sample also waits for sysinfo's CPU delta window)
- Per-workflow wait timeout: 120s
- Engine version: 0.1.0
- Store backend: memory
- Error-rate gate: 0.000%

Backend selected by --store; connection URLs are not copied into this report; closed-loop load; sampled peaks may understate instantaneous peaks. Run a release build on the target machine.

## Per-phase results

| Phase | Concurrency | Attempted | Completed | Errored | Terminated | Throughput/s | Error rate | Elapsed/s |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| warmup | 16 | 100 | 100 | 0 | 0 | 274.28 | 0.0000% | 0.36 |

**warmup start API latency (ms)**: p50 0.448 / p90 0.736 / p95 0.937 / p99 2.169 / p99.9 2.220 / max 2.220 / mean 0.538 / stddev 0.311
**warmup end-to-end latency (ms)**: p50 36.498 / p90 114.237 / p95 128.707 / p99 139.908 / p99.9 151.522 / max 151.522 / mean 52.738 / stddev 42.299

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 155.66 | 0.0000% | 642.42 |

**load-1/1-c16 start API latency (ms)**: p50 0.676 / p90 1.799 / p95 2.772 / p99 4.876 / p99.9 36.956 / max 232.817 / mean 1.075 / stddev 2.558
**load-1/1-c16 end-to-end latency (ms)**: p50 71.911 / p90 213.695 / p95 308.807 / p99 551.567 / p99.9 875.658 / max 1628.549 / mean 102.778 / stddev 107.162

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 159.16 | 0.0000% | 0.63 |

**post-stress-smoke start API latency (ms)**: p50 0.913 / p90 1.371 / p95 1.471 / p99 3.726 / p99.9 4.429 / max 4.429 / mean 1.015 / stddev 0.516
**post-stress-smoke end-to-end latency (ms)**: p50 51.060 / p90 226.803 / p95 274.128 / p99 306.364 / p99.9 308.155 / max 308.155 / mean 91.218 / stddev 82.617

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
