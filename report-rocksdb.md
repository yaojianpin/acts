# Acts million-run stress report

- Generated at (Unix seconds): 1793716275
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 2233.30s
- Measured throughput: 44.78 workflows/s
- Peak RSS: 2.53 GiB
- Min system available memory: 12.14 GiB
- Process CPU: peak 373.7% / sample mean 335.2%
- System CPU: peak 93.8% / sample mean 82.6%
- Max storage used: 0.29 GiB
- Peak process rows: 1068 (active 128)
- Process rows at the end (after the cleanup wait, when configured): 0
- Residual aggregate documents: 0
- Residual index documents: 0
- Logical CPUs: 4
- Degraded health samples: 0
- Metric collection failures: 0
- Engine shutdown: 0.170s

## Test configuration

- Workload: workflow: init + variable while-loop + expression evaluation + two-way branch + finalize
- Host: linux / x86_64 (4 logical CPUs)
- Concurrency stages: [16, 32, 64, 128]
- Warmup workflows: 100
- Steady-state soak: 0s
- Cleanup drain wait: 300s
- Scheduler workers: 4
- Store writer workers: 4
- Store shards: 4
- Cache cap: 320
- Scheduler queue cap: 4096
- Store writer queue cap: 4096
- Metric sample interval: 1000ms (each status sample also waits for sysinfo's CPU delta window)
- Per-workflow wait timeout: 120s
- Engine version: 0.1.0
- Store backend: rocksdb (target/stress-results/rocksdb-baseline-store, 4 shards)
- Error-rate gate: 0.000%

Backend selected by --store; connection URLs are not copied into this report; closed-loop load; sampled peaks may understate instantaneous peaks. Run a release build on the target machine.

## Per-phase results

| Phase | Concurrency | Attempted | Completed | Errored | Terminated | Throughput/s | Error rate | Elapsed/s |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| warmup | 16 | 100 | 100 | 0 | 0 | 69.15 | 0.0000% | 1.45 |

**warmup start API latency (ms)**: p50 4.562 / p90 12.128 / p95 16.104 / p99 17.813 / p99.9 18.198 / max 18.198 / mean 6.184 / stddev 3.773
**warmup end-to-end latency (ms)**: p50 203.967 / p90 318.058 / p95 362.802 / p99 384.363 / p99.9 404.561 / max 404.561 / mean 208.728 / stddev 85.371

| load-1/4-c16 | 16 | 25000 | 25000 | 0 | 0 | 45.80 | 0.0000% | 545.82 |

**load-1/4-c16 start API latency (ms)**: p50 6.978 / p90 12.435 / p95 13.666 / p99 15.817 / p99.9 20.038 / max 49.713 / mean 7.859 / stddev 3.130
**load-1/4-c16 end-to-end latency (ms)**: p50 277.201 / p90 726.958 / p95 879.224 / p99 1198.951 / p99.9 1521.964 / max 1749.027 / mean 349.218 / stddev 266.211

| load-2/4-c32 | 32 | 25000 | 25000 | 0 | 0 | 44.20 | 0.0000% | 565.63 |

**load-2/4-c32 start API latency (ms)**: p50 8.284 / p90 13.708 / p95 14.924 / p99 17.697 / p99.9 26.971 / max 206.126 / mean 8.989 / stddev 3.831
**load-2/4-c32 end-to-end latency (ms)**: p50 584.303 / p90 1523.344 / p95 1823.544 / p99 2423.782 / p99.9 2999.446 / max 3303.412 / mean 723.562 / stddev 558.896

| load-3/4-c64 | 64 | 25000 | 25000 | 0 | 0 | 44.85 | 0.0000% | 557.42 |

**load-3/4-c64 start API latency (ms)**: p50 8.664 / p90 13.878 / p95 15.142 / p99 17.993 / p99.9 60.776 / max 88.736 / mean 9.287 / stddev 4.139
**load-3/4-c64 end-to-end latency (ms)**: p50 1204.750 / p90 2914.439 / p95 3531.230 / p99 4772.960 / p99.9 6485.834 / max 7565.013 / mean 1424.307 / stddev 1084.863

| load-4/4-c128 | 128 | 25000 | 25000 | 0 | 0 | 44.29 | 0.0000% | 564.42 |

**load-4/4-c128 start API latency (ms)**: p50 8.941 / p90 14.556 / p95 15.932 / p99 19.478 / p99.9 133.791 / max 394.341 / mean 9.956 / stddev 8.079
**load-4/4-c128 end-to-end latency (ms)**: p50 2415.619 / p90 6006.659 / p95 7234.950 / p99 9658.744 / p99.9 11824.193 / max 12594.340 / mean 2885.559 / stddev 2242.318

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 38.34 | 0.0000% | 2.61 |

**post-stress-smoke start API latency (ms)**: p50 7.756 / p90 14.404 / p95 15.645 / p99 16.650 / p99.9 17.148 / max 17.148 / mean 9.170 / stddev 3.621
**post-stress-smoke end-to-end latency (ms)**: p50 273.712 / p90 814.197 / p95 986.703 / p99 1084.934 / p99.9 1108.710 / max 1108.710 / mean 374.669 / stddev 278.774

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
