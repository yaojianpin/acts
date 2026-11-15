# Acts million-run stress report

- Generated at (Unix seconds): 1794727616
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 868.05s
- Measured throughput: 115.20 workflows/s
- Peak RSS: 0.25 GiB
- Min system available memory: 14.42 GiB
- Process CPU: peak 389.6% / sample mean 318.1%
- System CPU: peak 97.6% / sample mean 81.7%
- Max storage used: 0.14 GiB
- Peak process rows: 2111 (active 17)
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
| warmup | 16 | 100 | 100 | 0 | 0 | 171.50 | 0.0000% | 0.58 |

**warmup start API latency (ms)**: p50 0.745 / p90 1.081 / p95 1.395 / p99 2.218 / p99.9 3.361 / max 3.361 / mean 0.847 / stddev 0.357
**warmup end-to-end latency (ms)**: p50 82.725 / p90 156.451 / p95 179.398 / p99 201.191 / p99.9 215.571 / max 215.571 / mean 85.533 / stddev 50.600

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 115.20 | 0.0000% | 868.05 |

**load-1/1-c16 start API latency (ms)**: p50 0.933 / p90 1.997 / p95 2.733 / p99 4.593 / p99.9 44.432 / max 158.901 / mean 1.333 / stddev 2.780
**load-1/1-c16 end-to-end latency (ms)**: p50 109.141 / p90 284.508 / p95 355.603 / p99 553.002 / p99.9 804.460 / max 1250.031 / mean 138.877 / stddev 115.060

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 143.70 | 0.0000% | 0.70 |

**post-stress-smoke start API latency (ms)**: p50 0.832 / p90 1.237 / p95 1.483 / p99 2.709 / p99.9 2.903 / max 2.903 / mean 0.945 / stddev 0.384
**post-stress-smoke end-to-end latency (ms)**: p50 100.210 / p90 165.046 / p95 206.684 / p99 228.240 / p99.9 251.617 / max 251.617 / mean 101.971 / stddev 52.300

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
