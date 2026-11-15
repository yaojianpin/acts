# Acts million-run stress report

- Generated at (Unix seconds): 1794733485
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 1497.98s
- Measured throughput: 66.76 workflows/s
- Peak RSS: 0.75 GiB
- Min system available memory: 13.87 GiB
- Process CPU: peak 323.3% / sample mean 288.3%
- System CPU: peak 77.0% / sample mean 70.3%
- Max storage used: 1.15 GiB
- Peak process rows: 1119 (active 16)
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
| warmup | 16 | 100 | 100 | 0 | 0 | 73.72 | 0.0000% | 1.36 |

**warmup start API latency (ms)**: p50 5.395 / p90 10.709 / p95 14.278 / p99 14.741 / p99.9 16.248 / max 16.248 / mean 6.308 / stddev 3.007
**warmup end-to-end latency (ms)**: p50 172.848 / p90 359.471 / p95 466.743 / p99 512.218 / p99.9 539.832 / max 539.832 / mean 206.142 / stddev 119.990

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 66.76 | 0.0000% | 1497.98 |

**load-1/1-c16 start API latency (ms)**: p50 5.465 / p90 7.731 / p95 8.526 / p99 10.467 / p99.9 16.930 / max 389.194 / mean 5.697 / stddev 2.374
**load-1/1-c16 end-to-end latency (ms)**: p50 197.159 / p90 484.949 / p95 579.233 / p99 758.877 / p99.9 919.432 / max 1123.286 / mean 239.666 / stddev 170.995

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 53.76 | 0.0000% | 1.86 |

**post-stress-smoke start API latency (ms)**: p50 7.529 / p90 14.103 / p95 14.863 / p99 16.579 / p99.9 17.720 / max 17.720 / mean 8.273 / stddev 3.376
**post-stress-smoke end-to-end latency (ms)**: p50 251.805 / p90 511.227 / p95 576.964 / p99 641.616 / p99.9 680.431 / max 680.431 / mean 277.044 / stddev 159.087

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
