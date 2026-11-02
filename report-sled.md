# Acts million-run stress report

- Generated at (Unix seconds): 1793625927
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 2041.19s
- Measured throughput: 48.99 workflows/s
- Peak RSS: 0.34 GiB
- Min system available memory: 14.33 GiB
- Process CPU: peak 357.3% / sample mean 317.6%
- System CPU: peak 95.0% / sample mean 80.5%
- Max storage used: 0.97 GiB
- Peak process rows: 895 (active 128)
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
| warmup | 16 | 100 | 100 | 0 | 0 | 53.30 | 0.0000% | 1.88 |

**warmup start API latency (ms)**: p50 5.099 / p90 17.393 / p95 19.987 / p99 22.515 / p99.9 23.049 / max 23.049 / mean 7.169 / stddev 5.204
**warmup end-to-end latency (ms)**: p50 198.618 / p90 693.810 / p95 779.415 / p99 880.010 / p99.9 887.427 / max 887.427 / mean 283.528 / stddev 234.943

| load-1/4-c16 | 16 | 25000 | 25000 | 0 | 0 | 49.33 | 0.0000% | 506.83 |

**load-1/4-c16 start API latency (ms)**: p50 5.243 / p90 7.249 / p95 7.940 / p99 9.472 / p99.9 12.893 / max 22.742 / mean 5.422 / stddev 1.426
**load-1/4-c16 end-to-end latency (ms)**: p50 265.541 / p90 664.862 / p95 790.243 / p99 1013.348 / p99.9 1199.434 / max 1315.329 / mean 324.288 / stddev 231.940

| load-2/4-c32 | 32 | 25000 | 25000 | 0 | 0 | 48.73 | 0.0000% | 512.98 |

**load-2/4-c32 start API latency (ms)**: p50 5.750 / p90 7.870 / p95 8.538 / p99 10.060 / p99.9 17.862 / max 37.962 / mean 5.932 / stddev 1.619
**load-2/4-c32 end-to-end latency (ms)**: p50 531.542 / p90 1382.944 / p95 1679.002 / p99 2187.771 / p99.9 2646.305 / max 3032.008 / mean 656.265 / stddev 506.890

| load-3/4-c64 | 64 | 25000 | 25000 | 0 | 0 | 49.68 | 0.0000% | 503.17 |

**load-3/4-c64 start API latency (ms)**: p50 5.858 / p90 7.927 / p95 8.610 / p99 10.200 / p99.9 62.774 / max 87.044 / mean 6.129 / stddev 3.065
**load-3/4-c64 end-to-end latency (ms)**: p50 1081.231 / p90 2683.372 / p95 3265.274 / p99 4356.640 / p99.9 5344.739 / max 6176.896 / mean 1286.988 / stddev 997.547

| load-4/4-c128 | 128 | 25000 | 25000 | 0 | 0 | 48.24 | 0.0000% | 518.21 |

**load-4/4-c128 start API latency (ms)**: p50 6.226 / p90 8.370 / p95 9.107 / p99 11.138 / p99.9 116.886 / max 134.521 / mean 6.775 / stddev 6.149
**load-4/4-c128 end-to-end latency (ms)**: p50 2355.601 / p90 5239.596 / p95 6124.686 / p99 7881.418 / p99.9 9722.258 / max 10701.481 / mean 2647.857 / stddev 1829.705

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 37.66 | 0.0000% | 2.66 |

**post-stress-smoke start API latency (ms)**: p50 7.197 / p90 18.867 / p95 24.158 / p99 26.212 / p99.9 26.323 / max 26.323 / mean 9.359 / stddev 5.650
**post-stress-smoke end-to-end latency (ms)**: p50 357.661 / p90 698.645 / p95 741.448 / p99 878.571 / p99.9 879.199 / max 879.199 / mean 391.925 / stddev 215.636

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
