# Acts million-run stress report

- Generated at (Unix seconds): 1794733905
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 1848.59s
- Measured throughput: 54.10 workflows/s
- Peak RSS: 2.28 GiB
- Min system available memory: 12.30 GiB
- Process CPU: peak 354.4% / sample mean 307.6%
- System CPU: peak 88.6% / sample mean 76.2%
- Max storage used: 0.28 GiB
- Peak process rows: 1228 (active 16)
- Process rows at the end (after the cleanup wait, when configured): 0
- Residual aggregate documents: 0
- Residual index documents: 0
- Logical CPUs: 4
- Degraded health samples: 0
- Metric collection failures: 0
- Engine shutdown: 0.461s

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
| warmup | 16 | 100 | 100 | 0 | 0 | 87.44 | 0.0000% | 1.14 |

**warmup start API latency (ms)**: p50 3.704 / p90 8.429 / p95 9.345 / p99 10.934 / p99.9 11.283 / max 11.283 / mean 4.577 / stddev 2.144
**warmup end-to-end latency (ms)**: p50 159.835 / p90 295.647 / p95 321.938 / p99 360.138 / p99.9 371.514 / max 371.514 / mean 172.977 / stddev 74.720

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 54.10 | 0.0000% | 1848.59 |

**load-1/1-c16 start API latency (ms)**: p50 5.458 / p90 9.796 / p95 10.833 / p99 12.885 / p99.9 16.569 / max 384.359 / mean 6.231 / stddev 2.706
**load-1/1-c16 end-to-end latency (ms)**: p50 234.287 / p90 612.728 / p95 753.931 / p99 1026.865 / p99.9 1268.294 / max 1592.778 / mean 295.754 / stddev 225.366

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 56.31 | 0.0000% | 1.78 |

**post-stress-smoke start API latency (ms)**: p50 6.152 / p90 8.127 / p95 8.444 / p99 9.121 / p99.9 9.899 / max 9.899 / mean 6.220 / stddev 1.480
**post-stress-smoke end-to-end latency (ms)**: p50 211.065 / p90 560.499 / p95 693.864 / p99 844.373 / p99.9 877.041 / max 877.041 / mean 264.832 / stddev 191.611

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
