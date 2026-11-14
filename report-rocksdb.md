# Acts million-run stress report

- Generated at (Unix seconds): 1794661101
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 1516.84s
- Measured throughput: 65.93 workflows/s
- Peak RSS: 2.33 GiB
- Min system available memory: 12.32 GiB
- Process CPU: peak 359.0% / sample mean 302.0%
- System CPU: peak 88.5% / sample mean 75.3%
- Max storage used: 0.29 GiB
- Peak process rows: 1783 (active 16)
- Process rows at the end (after the cleanup wait, when configured): 0
- Residual aggregate documents: 0
- Residual index documents: 0
- Logical CPUs: 4
- Degraded health samples: 0
- Metric collection failures: 0
- Engine shutdown: 0.725s

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
| warmup | 16 | 100 | 100 | 0 | 0 | 123.44 | 0.0000% | 0.81 |

**warmup start API latency (ms)**: p50 2.404 / p90 5.631 / p95 6.091 / p99 6.744 / p99.9 6.744 / max 6.744 / mean 3.037 / stddev 1.402
**warmup end-to-end latency (ms)**: p50 116.809 / p90 224.026 / p95 255.855 / p99 280.767 / p99.9 285.308 / max 285.308 / mean 122.333 / stddev 69.133

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 65.93 | 0.0000% | 1516.84 |

**load-1/1-c16 start API latency (ms)**: p50 4.579 / p90 9.135 / p95 10.271 / p99 12.885 / p99.9 17.699 / max 105.458 / mean 5.386 / stddev 2.740
**load-1/1-c16 end-to-end latency (ms)**: p50 185.233 / p90 520.100 / p95 646.185 / p99 880.666 / p99.9 1121.436 / max 1457.467 / mean 242.672 / stddev 195.377

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 105.61 | 0.0000% | 0.95 |

**post-stress-smoke start API latency (ms)**: p50 2.999 / p90 7.875 / p95 8.255 / p99 13.033 / p99.9 13.309 / max 13.309 / mean 3.872 / stddev 2.232
**post-stress-smoke end-to-end latency (ms)**: p50 130.854 / p90 257.764 / p95 274.242 / p99 325.210 / p99.9 349.066 / max 349.066 / mean 142.683 / stddev 81.173

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
