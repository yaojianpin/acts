# Acts million-run stress report

- Generated at (Unix seconds): 1794516079
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 865.53s
- Measured throughput: 115.54 workflows/s
- Peak RSS: 0.24 GiB
- Min system available memory: 14.37 GiB
- Process CPU: peak 379.5% / sample mean 321.6%
- System CPU: peak 94.3% / sample mean 81.7%
- Max storage used: 0.14 GiB
- Peak process rows: 2059 (active 17)
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
| warmup | 16 | 100 | 100 | 0 | 0 | 167.06 | 0.0000% | 0.60 |

**warmup start API latency (ms)**: p50 0.789 / p90 2.209 / p95 3.025 / p99 3.742 / p99.9 4.249 / max 4.249 / mean 1.078 / stddev 0.733
**warmup end-to-end latency (ms)**: p50 77.602 / p90 155.537 / p95 190.954 / p99 245.653 / p99.9 249.798 / max 249.798 / mean 88.560 / stddev 51.493

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 115.54 | 0.0000% | 865.53 |

**load-1/1-c16 start API latency (ms)**: p50 1.029 / p90 1.766 / p95 2.166 / p99 3.262 / p99.9 39.464 / max 108.556 / mean 1.324 / stddev 2.417
**load-1/1-c16 end-to-end latency (ms)**: p50 112.582 / p90 283.703 / p95 343.795 / p99 489.637 / p99.9 646.092 / max 814.603 / mean 138.469 / stddev 105.961

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 118.36 | 0.0000% | 0.84 |

**post-stress-smoke start API latency (ms)**: p50 0.977 / p90 1.410 / p95 1.510 / p99 1.878 / p99.9 3.223 / max 3.223 / mean 1.074 / stddev 0.319
**post-stress-smoke end-to-end latency (ms)**: p50 117.570 / p90 203.562 / p95 234.218 / p99 277.679 / p99.9 279.124 / max 279.124 / mean 118.737 / stddev 66.119

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
