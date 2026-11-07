# Acts million-run stress report

- Generated at (Unix seconds): 1794015442
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 1817.74s
- Measured throughput: 55.01 workflows/s
- Peak RSS: 2.46 GiB
- Min system available memory: 12.21 GiB
- Process CPU: peak 362.2% / sample mean 315.9%
- System CPU: peak 89.7% / sample mean 77.2%
- Max storage used: 0.29 GiB
- Peak process rows: 1373 (active 16)
- Process rows at the end (after the cleanup wait, when configured): 0
- Residual aggregate documents: 0
- Residual index documents: 0
- Logical CPUs: 4
- Degraded health samples: 0
- Metric collection failures: 0
- Engine shutdown: 0.583s

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
| warmup | 16 | 100 | 100 | 0 | 0 | 91.69 | 0.0000% | 1.09 |

**warmup start API latency (ms)**: p50 3.088 / p90 8.941 / p95 10.464 / p99 12.723 / p99.9 14.286 / max 14.286 / mean 4.160 / stddev 2.676
**warmup end-to-end latency (ms)**: p50 163.937 / p90 264.130 / p95 280.585 / p99 313.501 / p99.9 346.309 / max 346.309 / mean 162.256 / stddev 75.857

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 55.01 | 0.0000% | 1817.74 |

**load-1/1-c16 start API latency (ms)**: p50 5.467 / p90 10.258 / p95 11.421 / p99 13.846 / p99.9 19.054 / max 677.634 / mean 6.349 / stddev 4.733
**load-1/1-c16 end-to-end latency (ms)**: p50 228.635 / p90 610.509 / p95 753.287 / p99 1017.872 / p99.9 1301.528 / max 1570.156 / mean 290.812 / stddev 225.966

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 41.96 | 0.0000% | 2.38 |

**post-stress-smoke start API latency (ms)**: p50 9.488 / p90 13.346 / p95 13.913 / p99 16.686 / p99.9 21.215 / max 21.215 / mean 9.157 / stddev 3.345
**post-stress-smoke end-to-end latency (ms)**: p50 324.861 / p90 632.985 / p95 772.952 / p99 902.868 / p99.9 907.390 / max 907.390 / mean 351.257 / stddev 215.248

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
