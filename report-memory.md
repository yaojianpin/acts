# Acts million-run stress report

- Generated at (Unix seconds): 1793714659
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 789.77s
- Measured throughput: 126.62 workflows/s
- Peak RSS: 0.35 GiB
- Min system available memory: 14.33 GiB
- Process CPU: peak 385.5% / sample mean 312.0%
- System CPU: peak 96.5% / sample mean 79.2%
- Max storage used: 0.16 GiB
- Peak process rows: 2464 (active 129)
- Process rows at the end (after the cleanup wait, when configured): 0
- Residual aggregate documents: 0
- Residual index documents: 0
- Logical CPUs: 4
- Degraded health samples: 0
- Metric collection failures: 0
- Engine shutdown: 0.002s

## Test configuration

- Workload: workflow: init + variable while-loop + expression evaluation + two-way branch + finalize
- Host: linux / x86_64 (4 logical CPUs)
- Concurrency stages: [16, 32, 64, 128]
- Warmup workflows: 100
- Steady-state soak: 0s
- Cleanup drain wait: 300s
- Scheduler workers: 4
- Store writer workers: 4
- Store shards: 1
- Cache cap: 320
- Scheduler queue cap: 4096
- Store writer queue cap: 4096
- Metric sample interval: 1000ms (each status sample also waits for sysinfo's CPU delta window)
- Per-workflow wait timeout: 120s
- Engine version: 0.1.0
- Store backend: memory
- Error-rate gate: 0.000%

Backend selected by --store; connection URLs are not copied into this report; closed-loop load; sampled peaks may understate instantaneous peaks. Run a release build on the target machine.

## Per-phase results

| Phase | Concurrency | Attempted | Completed | Errored | Terminated | Throughput/s | Error rate | Elapsed/s |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| warmup | 16 | 100 | 100 | 0 | 0 | 206.63 | 0.0000% | 0.48 |

**warmup start API latency (ms)**: p50 0.582 / p90 0.849 / p95 1.056 / p99 1.570 / p99.9 2.091 / max 2.091 / mean 0.660 / stddev 0.255
**warmup end-to-end latency (ms)**: p50 30.915 / p90 187.321 / p95 215.250 / p99 237.361 / p99.9 239.634 / max 239.634 / mean 69.091 / stddev 72.189

| load-1/4-c16 | 16 | 25000 | 25000 | 0 | 0 | 120.44 | 0.0000% | 207.57 |

**load-1/4-c16 start API latency (ms)**: p50 0.920 / p90 2.310 / p95 3.277 / p99 4.820 / p99.9 31.421 / max 145.627 / mean 1.316 / stddev 2.385
**load-1/4-c16 end-to-end latency (ms)**: p50 100.804 / p90 267.528 / p95 359.196 / p99 610.343 / p99.9 936.863 / max 1123.686 / mean 132.816 / stddev 121.301

| load-2/4-c32 | 32 | 25000 | 25000 | 0 | 0 | 125.74 | 0.0000% | 198.82 |

**load-2/4-c32 start API latency (ms)**: p50 0.860 / p90 2.065 / p95 2.948 / p99 4.232 / p99.9 36.193 / max 151.474 / mean 1.224 / stddev 2.255
**load-2/4-c32 end-to-end latency (ms)**: p50 186.446 / p90 544.113 / p95 708.105 / p99 1217.487 / p99.9 1700.103 / max 2205.186 / mean 254.220 / stddev 242.509

| load-3/4-c64 | 64 | 25000 | 25000 | 0 | 0 | 132.29 | 0.0000% | 188.98 |

**load-3/4-c64 start API latency (ms)**: p50 0.817 / p90 1.964 / p95 2.715 / p99 3.807 / p99.9 24.909 / max 117.531 / mean 1.128 / stddev 1.767
**load-3/4-c64 end-to-end latency (ms)**: p50 348.120 / p90 1049.490 / p95 1394.174 / p99 2278.742 / p99.9 3177.049 / max 3804.316 / mean 483.283 / stddev 468.152

| load-4/4-c128 | 128 | 25000 | 25000 | 0 | 0 | 128.60 | 0.0000% | 194.41 |

**load-4/4-c128 start API latency (ms)**: p50 0.835 / p90 2.117 / p95 2.947 / p99 3.954 / p99.9 17.586 / max 62.455 / mean 1.150 / stddev 1.294
**load-4/4-c128 end-to-end latency (ms)**: p50 784.833 / p90 2083.606 / p95 2676.270 / p99 4038.448 / p99.9 5282.296 / max 6115.195 / mean 993.773 / stddev 846.859

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 178.05 | 0.0000% | 0.56 |

**post-stress-smoke start API latency (ms)**: p50 0.681 / p90 0.960 / p95 1.023 / p99 1.346 / p99.9 1.751 / max 1.751 / mean 0.738 / stddev 0.189
**post-stress-smoke end-to-end latency (ms)**: p50 77.197 / p90 134.492 / p95 148.643 / p99 172.582 / p99.9 180.061 / max 180.061 / mean 81.378 / stddev 40.769

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
