# Acts million-run stress report

- Generated at (Unix seconds): 1795044724
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 1842.09s
- Measured throughput: 54.29 workflows/s
- Peak RSS: 2.23 GiB
- Min system available memory: 12.37 GiB
- Process CPU: peak 368.4% / sample mean 319.2%
- System CPU: peak 89.9% / sample mean 79.8%
- Max storage used: 0.29 GiB
- Peak process rows: 1364 (active 16)
- Process rows at the end (after the cleanup wait, when configured): 0
- Residual aggregate documents: 0
- Residual index documents: 0
- Logical CPUs: 4
- Degraded health samples: 0
- Metric collection failures: 0
- Engine shutdown: 0.687s

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
- Store backend: rocksdb (target/stress-results/rocksdb-baseline-store, 4 shards)
- Error-rate gate: 0.000%

Backend selected by --store; connection URLs are not copied into this report; closed-loop load; sampled peaks may understate instantaneous peaks. Run a release build on the target machine.

## Per-phase results

| Phase | Concurrency | Attempted | Completed | Errored | Terminated | Throughput/s | Error rate | Elapsed/s |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| warmup | 16 | 100 | 100 | 0 | 0 | 99.64 | 0.0000% | 1.00 |

**warmup start API latency (ms)**: p50 3.297 / p90 6.468 / p95 7.450 / p99 8.570 / p99.9 8.619 / max 8.619 / mean 3.859 / stddev 1.572
**warmup end-to-end latency (ms)**: p50 146.036 / p90 237.740 / p95 253.717 / p99 301.833 / p99.9 311.193 / max 311.193 / mean 152.699 / stddev 61.890

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 54.29 | 0.0000% | 1842.09 |

**load-1/1-c16 start API latency (ms)**: p50 5.551 / p90 10.437 / p95 11.573 / p99 14.039 / p99.9 18.897 / max 423.279 / mean 6.419 / stddev 3.235
**load-1/1-c16 end-to-end latency (ms)**: p50 230.666 / p90 620.830 / p95 764.971 / p99 1050.244 / p99.9 1304.101 / max 1775.713 / mean 294.719 / stddev 230.871

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 82.74 | 0.0000% | 1.21 |

**post-stress-smoke start API latency (ms)**: p50 4.184 / p90 6.825 / p95 11.398 / p99 12.012 / p99.9 12.264 / max 12.264 / mean 4.899 / stddev 2.268
**post-stress-smoke end-to-end latency (ms)**: p50 177.570 / p90 325.421 / p95 356.085 / p99 380.032 / p99.9 413.745 / max 413.745 / mean 181.065 / stddev 102.879

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
