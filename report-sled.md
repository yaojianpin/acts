# Acts million-run stress report

- Generated at (Unix seconds): 1793972540
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 2076.49s
- Measured throughput: 48.16 workflows/s
- Peak RSS: 0.75 GiB
- Min system available memory: 13.85 GiB
- Process CPU: peak 345.9% / sample mean 307.2%
- System CPU: peak 83.3% / sample mean 75.4%
- Max storage used: 0.70 GiB
- Peak process rows: 825 (active 16)
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
| warmup | 16 | 100 | 100 | 0 | 0 | 55.75 | 0.0000% | 1.79 |

**warmup start API latency (ms)**: p50 7.102 / p90 16.345 / p95 18.202 / p99 19.890 / p99.9 20.723 / max 20.723 / mean 8.580 / stddev 4.109
**warmup end-to-end latency (ms)**: p50 246.585 / p90 510.639 / p95 541.013 / p99 589.351 / p99.9 637.099 / max 637.099 / mean 270.472 / stddev 153.388

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 48.16 | 0.0000% | 2076.49 |

**load-1/1-c16 start API latency (ms)**: p50 7.048 / p90 9.447 / p95 10.209 / p99 11.795 / p99.9 67.983 / max 391.459 / mean 7.406 / stddev 7.187
**load-1/1-c16 end-to-end latency (ms)**: p50 274.545 / p90 671.514 / p95 803.945 / p99 1024.123 / p99.9 1216.259 / max 1451.454 / mean 332.217 / stddev 235.301

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 51.24 | 0.0000% | 1.95 |

**post-stress-smoke start API latency (ms)**: p50 6.511 / p90 17.210 / p95 19.983 / p99 23.060 / p99.9 23.931 / max 23.931 / mean 8.371 / stddev 4.659
**post-stress-smoke end-to-end latency (ms)**: p50 233.000 / p90 686.282 / p95 762.961 / p99 836.032 / p99.9 850.599 / max 850.599 / mean 291.888 / stddev 215.832

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
