# Acts million-run stress report

- Generated at (Unix seconds): 1794963097
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 1591.71s
- Measured throughput: 62.83 workflows/s
- Peak RSS: 0.80 GiB
- Min system available memory: 13.85 GiB
- Process CPU: peak 333.3% / sample mean 289.4%
- System CPU: peak 82.7% / sample mean 70.5%
- Max storage used: 1.47 GiB
- Peak process rows: 1222 (active 16)
- Process rows at the end (after the cleanup wait, when configured): 0
- Residual aggregate documents: 0
- Residual index documents: 0
- Logical CPUs: 4
- Degraded health samples: 0
- Metric collection failures: 0
- Engine shutdown: 0.000s

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
- Store backend: sled (target/stress-results/sled-baseline-store, 4 shards, 50ms flush)
- Error-rate gate: 0.000%

Backend selected by --store; connection URLs are not copied into this report; closed-loop load; sampled peaks may understate instantaneous peaks. Run a release build on the target machine.

## Per-phase results

| Phase | Concurrency | Attempted | Completed | Errored | Terminated | Throughput/s | Error rate | Elapsed/s |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| warmup | 16 | 100 | 100 | 0 | 0 | 81.19 | 0.0000% | 1.23 |

**warmup start API latency (ms)**: p50 4.944 / p90 11.920 / p95 16.109 / p99 17.884 / p99.9 18.877 / max 18.877 / mean 6.439 / stddev 3.906
**warmup end-to-end latency (ms)**: p50 165.273 / p90 326.936 / p95 422.749 / p99 475.380 / p99.9 481.353 / max 481.353 / mean 185.342 / stddev 111.548

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 62.83 | 0.0000% | 1591.71 |

**load-1/1-c16 start API latency (ms)**: p50 5.774 / p90 8.262 / p95 9.122 / p99 11.217 / p99.9 18.997 / max 156.993 / mean 6.034 / stddev 2.404
**load-1/1-c16 end-to-end latency (ms)**: p50 212.861 / p90 509.918 / p95 606.339 / p99 791.814 / p99.9 989.251 / max 1170.020 / mean 254.651 / stddev 178.839

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 46.22 | 0.0000% | 2.16 |

**post-stress-smoke start API latency (ms)**: p50 7.820 / p90 13.966 / p95 15.702 / p99 17.380 / p99.9 18.918 / max 18.918 / mean 8.607 / stddev 3.187
**post-stress-smoke end-to-end latency (ms)**: p50 238.746 / p90 726.193 / p95 847.661 / p99 1004.452 / p99.9 1009.212 / max 1009.212 / mean 322.428 / stddev 244.557

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
