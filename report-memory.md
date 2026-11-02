# Acts million-run stress report

- Generated at (Unix seconds): 1793624822
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 931.24s
- Measured throughput: 107.38 workflows/s
- Peak RSS: 0.29 GiB
- Min system available memory: 14.42 GiB
- Process CPU: peak 390.7% / sample mean 319.0%
- System CPU: peak 96.4% / sample mean 81.3%
- Max storage used: 0.13 GiB
- Peak process rows: 2009 (active 129)
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
- Concurrency stages: [16, 32, 64, 128]
- Warmup workflows: 100
- Steady-state soak: 0s
- Cleanup drain wait: 300s
- Scheduler workers: 4
- Store writer workers: 4
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
| warmup | 16 | 100 | 100 | 0 | 0 | 170.72 | 0.0000% | 0.59 |

**warmup start API latency (ms)**: p50 0.758 / p90 1.230 / p95 2.177 / p99 3.296 / p99.9 3.314 / max 3.314 / mean 0.931 / stddev 0.525
**warmup end-to-end latency (ms)**: p50 80.234 / p90 152.939 / p95 175.615 / p99 191.910 / p99.9 198.960 / max 198.960 / mean 86.296 / stddev 51.153

| load-1/4-c16 | 16 | 25000 | 25000 | 0 | 0 | 109.83 | 0.0000% | 227.62 |

**load-1/4-c16 start API latency (ms)**: p50 0.990 / p90 2.245 / p95 3.184 / p99 4.723 / p99.9 42.494 / max 152.053 / mean 1.408 / stddev 2.798
**load-1/4-c16 end-to-end latency (ms)**: p50 112.055 / p90 298.657 / p95 384.601 / p99 627.493 / p99.9 905.981 / max 1111.645 / mean 145.621 / stddev 126.835

| load-2/4-c32 | 32 | 25000 | 25000 | 0 | 0 | 108.02 | 0.0000% | 231.43 |

**load-2/4-c32 start API latency (ms)**: p50 1.010 / p90 2.252 / p95 3.063 / p99 4.333 / p99.9 40.652 / max 148.694 / mean 1.389 / stddev 2.484
**load-2/4-c32 end-to-end latency (ms)**: p50 230.050 / p90 633.353 / p95 795.132 / p99 1191.247 / p99.9 1572.055 / max 1857.269 / mean 296.079 / stddev 252.621

| load-3/4-c64 | 64 | 25000 | 25000 | 0 | 0 | 105.13 | 0.0000% | 237.79 |

**load-3/4-c64 start API latency (ms)**: p50 1.028 / p90 2.394 / p95 3.230 / p99 4.410 / p99.9 30.230 / max 107.302 / mean 1.392 / stddev 1.888
**load-3/4-c64 end-to-end latency (ms)**: p50 473.419 / p90 1288.877 / p95 1627.729 / p99 2347.125 / p99.9 3272.824 / max 4276.200 / mean 607.960 / stddev 510.526

| load-4/4-c128 | 128 | 25000 | 25000 | 0 | 0 | 106.66 | 0.0000% | 234.40 |

**load-4/4-c128 start API latency (ms)**: p50 1.021 / p90 2.294 / p95 3.102 / p99 4.389 / p99.9 8.771 / max 162.884 / mean 1.347 / stddev 1.855
**load-4/4-c128 end-to-end latency (ms)**: p50 896.008 / p90 2657.742 / p95 3415.762 / p99 4889.863 / p99.9 6664.282 / max 7906.011 / mean 1195.795 / stddev 1070.014

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 60.88 | 0.0000% | 1.64 |

**post-stress-smoke start API latency (ms)**: p50 2.250 / p90 3.710 / p95 4.113 / p99 66.718 / p99.9 79.149 / max 79.149 / mean 3.631 / stddev 10.002
**post-stress-smoke end-to-end latency (ms)**: p50 251.579 / p90 401.675 / p95 461.064 / p99 524.775 / p99.9 581.003 / max 581.003 / mean 246.663 / stddev 138.719

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
