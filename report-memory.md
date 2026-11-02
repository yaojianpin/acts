# Acts million-run stress report

- Generated at (Unix seconds): 1793623432
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 898.20s
- Measured throughput: 111.33 workflows/s
- Peak RSS: 0.29 GiB
- Min system available memory: 14.41 GiB
- Process CPU: peak 389.7% / sample mean 324.2%
- System CPU: peak 96.6% / sample mean 82.7%
- Max storage used: 0.14 GiB
- Peak process rows: 2061 (active 129)
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
| warmup | 16 | 100 | 100 | 0 | 0 | 182.67 | 0.0000% | 0.55 |

**warmup start API latency (ms)**: p50 0.784 / p90 1.216 / p95 1.569 / p99 2.237 / p99.9 5.890 / max 5.890 / mean 0.940 / stddev 0.590
**warmup end-to-end latency (ms)**: p50 59.245 / p90 157.242 / p95 206.340 / p99 245.493 / p99.9 249.996 / max 249.996 / mean 81.635 / stddev 59.069

| load-1/4-c16 | 16 | 25000 | 25000 | 0 | 0 | 113.91 | 0.0000% | 219.48 |

**load-1/4-c16 start API latency (ms)**: p50 0.964 / p90 2.052 / p95 2.804 / p99 4.499 / p99.9 50.151 / max 155.411 / mean 1.363 / stddev 2.928
**load-1/4-c16 end-to-end latency (ms)**: p50 110.353 / p90 287.845 / p95 358.273 / p99 568.755 / p99.9 797.849 / max 1065.604 / mean 140.425 / stddev 116.843

| load-2/4-c32 | 32 | 25000 | 25000 | 0 | 0 | 108.86 | 0.0000% | 229.66 |

**load-2/4-c32 start API latency (ms)**: p50 1.007 / p90 2.197 / p95 3.009 / p99 4.438 / p99.9 37.480 / max 118.823 / mean 1.394 / stddev 2.562
**load-2/4-c32 end-to-end latency (ms)**: p50 224.777 / p90 624.904 / p95 789.691 / p99 1216.885 / p99.9 1628.250 / max 2130.038 / mean 293.774 / stddev 255.951

| load-3/4-c64 | 64 | 25000 | 25000 | 0 | 0 | 111.85 | 0.0000% | 223.52 |

**load-3/4-c64 start API latency (ms)**: p50 0.986 / p90 2.122 / p95 2.799 / p99 4.012 / p99.9 25.014 / max 174.121 / mean 1.291 / stddev 1.877
**load-3/4-c64 end-to-end latency (ms)**: p50 412.392 / p90 1268.668 / p95 1603.577 / p99 2498.801 / p99.9 3276.233 / max 3791.971 / mean 571.537 / stddev 526.950

| load-4/4-c128 | 128 | 25000 | 25000 | 0 | 0 | 110.84 | 0.0000% | 225.54 |

**load-4/4-c128 start API latency (ms)**: p50 0.995 / p90 2.196 / p95 2.892 / p99 4.072 / p99.9 20.037 / max 80.066 / mean 1.305 / stddev 1.522
**load-4/4-c128 end-to-end latency (ms)**: p50 991.541 / p90 2346.941 / p95 2861.490 / p99 3882.077 / p99.9 4908.023 / max 5361.171 / mean 1152.735 / stddev 887.102

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 102.11 | 0.0000% | 0.98 |

**post-stress-smoke start API latency (ms)**: p50 1.011 / p90 2.334 / p95 2.880 / p99 5.141 / p99.9 5.323 / max 5.323 / mean 1.382 / stddev 0.849
**post-stress-smoke end-to-end latency (ms)**: p50 130.594 / p90 281.998 / p95 366.193 / p99 410.187 / p99.9 436.300 / max 436.300 / mean 141.581 / stddev 105.003

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
