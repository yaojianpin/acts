# Acts million-run stress report

- Generated at (Unix seconds): 1794047363
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 2400.55s
- Measured throughput: 41.66 workflows/s
- Peak RSS: 2.38 GiB
- Min system available memory: 12.29 GiB
- Process CPU: peak 368.8% / sample mean 321.7%
- System CPU: peak 89.9% / sample mean 78.7%
- Max storage used: 0.27 GiB
- Peak process rows: 1079 (active 16)
- Process rows at the end (after the cleanup wait, when configured): 0
- Residual aggregate documents: 0
- Residual index documents: 0
- Logical CPUs: 4
- Degraded health samples: 0
- Metric collection failures: 0
- Engine shutdown: 0.134s

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
| warmup | 16 | 100 | 100 | 0 | 0 | 72.02 | 0.0000% | 1.39 |

**warmup start API latency (ms)**: p50 4.390 / p90 9.674 / p95 11.222 / p99 11.796 / p99.9 12.408 / max 12.408 / mean 5.267 / stddev 2.428
**warmup end-to-end latency (ms)**: p50 141.947 / p90 462.278 / p95 573.384 / p99 655.927 / p99.9 687.869 / max 687.869 / mean 209.220 / stddev 164.456

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 41.66 | 0.0000% | 2400.55 |

**load-1/1-c16 start API latency (ms)**: p50 6.920 / p90 12.955 / p95 14.277 / p99 16.756 / p99.9 21.250 / max 212.716 / mean 7.986 / stddev 3.432
**load-1/1-c16 end-to-end latency (ms)**: p50 305.201 / p90 799.423 / p95 984.464 / p99 1336.183 / p99.9 1677.808 / max 2023.288 / mean 384.063 / stddev 295.107

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 35.15 | 0.0000% | 2.85 |

**post-stress-smoke start API latency (ms)**: p50 10.672 / p90 13.765 / p95 14.288 / p99 15.710 / p99.9 15.995 / max 15.995 / mean 10.195 / stddev 2.741
**post-stress-smoke end-to-end latency (ms)**: p50 297.893 / p90 961.476 / p95 1122.834 / p99 1375.893 / p99.9 1474.897 / max 1474.897 / mean 435.477 / stddev 338.201

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
