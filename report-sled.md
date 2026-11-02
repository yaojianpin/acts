# Acts million-run stress report

- Generated at (Unix seconds): 1793624719
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 2171.51s
- Measured throughput: 46.05 workflows/s
- Peak RSS: 0.28 GiB
- Min system available memory: 14.39 GiB
- Process CPU: peak 351.4% / sample mean 297.0%
- System CPU: peak 84.0% / sample mean 73.8%
- Max storage used: 0.94 GiB
- Peak process rows: 909 (active 128)
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
- Store backend: sled (target/stress-results/sled-store, 50ms flush)
- Error-rate gate: 0.000%

Backend selected by --store; connection URLs are not copied into this report; closed-loop load; sampled peaks may understate instantaneous peaks. Run a release build on the target machine.

## Per-phase results

| Phase | Concurrency | Attempted | Completed | Errored | Terminated | Throughput/s | Error rate | Elapsed/s |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| warmup | 16 | 100 | 100 | 0 | 0 | 50.27 | 0.0000% | 1.99 |

**warmup start API latency (ms)**: p50 5.168 / p90 14.574 / p95 18.186 / p99 19.523 / p99.9 20.423 / max 20.423 / mean 6.938 / stddev 4.430
**warmup end-to-end latency (ms)**: p50 265.441 / p90 532.350 / p95 680.145 / p99 781.091 / p99.9 834.722 / max 834.722 / mean 298.350 / stddev 185.154

| load-1/4-c16 | 16 | 25000 | 25000 | 0 | 0 | 45.78 | 0.0000% | 546.06 |

**load-1/4-c16 start API latency (ms)**: p50 5.886 / p90 8.129 / p95 8.912 / p99 10.690 / p99.9 14.585 / max 25.549 / mean 6.079 / stddev 1.615
**load-1/4-c16 end-to-end latency (ms)**: p50 290.484 / p90 709.987 / p95 841.858 / p99 1055.835 / p99.9 1271.159 / max 1496.434 / mean 349.393 / stddev 245.625

| load-2/4-c32 | 32 | 25000 | 25000 | 0 | 0 | 45.46 | 0.0000% | 549.99 |

**load-2/4-c32 start API latency (ms)**: p50 6.267 / p90 8.688 / p95 9.582 / p99 11.676 / p99.9 23.302 / max 43.870 / mean 6.501 / stddev 1.955
**load-2/4-c32 end-to-end latency (ms)**: p50 560.852 / p90 1497.540 / p95 1795.475 / p99 2298.246 / p99.9 2715.818 / max 2922.304 / mean 703.705 / stddev 544.769

| load-3/4-c64 | 64 | 25000 | 25000 | 0 | 0 | 44.65 | 0.0000% | 559.87 |

**load-3/4-c64 start API latency (ms)**: p50 6.676 / p90 9.273 / p95 10.215 / p99 12.723 / p99.9 54.967 / max 77.183 / mean 7.013 / stddev 2.948
**load-3/4-c64 end-to-end latency (ms)**: p50 1238.124 / p90 2850.059 / p95 3413.812 / p99 4365.645 / p99.9 5204.355 / max 5730.863 / mean 1431.426 / stddev 1007.253

| load-4/4-c128 | 128 | 25000 | 25000 | 0 | 0 | 48.49 | 0.0000% | 515.59 |

**load-4/4-c128 start API latency (ms)**: p50 6.324 / p90 8.453 / p95 9.198 / p99 11.095 / p99.9 135.253 / max 157.123 / mean 6.920 / stddev 7.089
**load-4/4-c128 end-to-end latency (ms)**: p50 2284.473 / p90 5316.284 / p95 6375.420 / p99 8392.007 / p99.9 10403.435 / max 11341.026 / mean 2635.613 / stddev 1929.680

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 48.06 | 0.0000% | 2.08 |

**post-stress-smoke start API latency (ms)**: p50 5.406 / p90 15.725 / p95 17.737 / p99 18.878 / p99.9 19.465 / max 19.465 / mean 7.040 / stddev 4.305
**post-stress-smoke end-to-end latency (ms)**: p50 245.489 / p90 631.129 / p95 737.019 / p99 790.947 / p99.9 910.852 / max 910.852 / mean 305.473 / stddev 203.608

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
