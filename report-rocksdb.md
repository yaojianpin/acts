# Acts million-run stress report

- Generated at (Unix seconds): 1795692371
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 3192.07s
- Measured throughput: 31.33 workflows/s
- Peak RSS: 2.72 GiB
- Min system available memory: 11.80 GiB
- Process CPU: peak 368.4% / sample mean 312.9%
- System CPU: peak 88.5% / sample mean 77.0%
- Max storage used: 0.29 GiB
- Peak process rows: 863 (active 16)
- Process rows at the end (after the cleanup wait, when configured): 0
- Residual aggregate documents: 0
- Residual index documents: 0
- Logical CPUs: 4
- Degraded health samples: 0
- Metric collection failures: 0
- Engine shutdown: 0.290s

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
| warmup | 16 | 100 | 100 | 0 | 0 | 53.23 | 0.0000% | 1.88 |

**warmup start API latency (ms)**: p50 3.604 / p90 8.623 / p95 10.385 / p99 11.466 / p99.9 11.963 / max 11.963 / mean 4.670 / stddev 2.393
**warmup end-to-end latency (ms)**: p50 232.870 / p90 524.490 / p95 568.104 / p99 617.912 / p99.9 666.713 / max 666.713 / mean 275.615 / stddev 162.955

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 31.33 | 0.0000% | 3192.07 |

**load-1/1-c16 start API latency (ms)**: p50 5.956 / p90 12.526 / p95 14.428 / p99 17.799 / p99.9 22.348 / max 84.937 / mean 7.307 / stddev 3.444
**load-1/1-c16 end-to-end latency (ms)**: p50 416.401 / p90 1045.572 / p95 1263.165 / p99 1638.769 / p99.9 2011.959 / max 2375.681 / mean 510.700 / stddev 372.099

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 31.60 | 0.0000% | 3.16 |

**post-stress-smoke start API latency (ms)**: p50 5.335 / p90 16.642 / p95 17.278 / p99 19.150 / p99.9 20.111 / max 20.111 / mean 7.817 / stddev 4.666
**post-stress-smoke end-to-end latency (ms)**: p50 300.460 / p90 930.467 / p95 1185.193 / p99 1433.699 / p99.9 1450.066 / max 1450.066 / mean 450.220 / stddev 346.071

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
