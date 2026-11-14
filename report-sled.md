# Acts million-run stress report

- Generated at (Unix seconds): 1794630063
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 2193.11s
- Measured throughput: 45.60 workflows/s
- Peak RSS: 0.72 GiB
- Min system available memory: 13.91 GiB
- Process CPU: peak 355.6% / sample mean 302.3%
- System CPU: peak 84.8% / sample mean 75.9%
- Max storage used: 0.70 GiB
- Peak process rows: 750 (active 16)
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
| warmup | 16 | 100 | 100 | 0 | 0 | 51.09 | 0.0000% | 1.96 |

**warmup start API latency (ms)**: p50 7.332 / p90 18.658 / p95 23.333 / p99 27.593 / p99.9 29.095 / max 29.095 / mean 9.381 / stddev 5.754
**warmup end-to-end latency (ms)**: p50 265.838 / p90 520.143 / p95 569.638 / p99 656.180 / p99.9 660.123 / max 660.123 / mean 295.515 / stddev 147.872

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 45.60 | 0.0000% | 2193.11 |

**load-1/1-c16 start API latency (ms)**: p50 7.638 / p90 10.286 / p95 11.118 / p99 12.831 / p99.9 15.632 / max 183.972 / mean 7.822 / stddev 2.284
**load-1/1-c16 end-to-end latency (ms)**: p50 297.074 / p90 696.676 / p95 819.423 / p99 1040.464 / p99.9 1251.191 / max 1383.238 / mean 350.875 / stddev 239.796

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 40.90 | 0.0000% | 2.45 |

**post-stress-smoke start API latency (ms)**: p50 9.861 / p90 19.311 / p95 22.091 / p99 23.572 / p99.9 23.837 / max 23.837 / mean 11.252 / stddev 4.452
**post-stress-smoke end-to-end latency (ms)**: p50 330.850 / p90 654.917 / p95 768.610 / p99 891.971 / p99.9 922.873 / max 922.873 / mean 375.663 / stddev 206.146

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
