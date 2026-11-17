# Acts million-run stress report

- Generated at (Unix seconds): 1794875918
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 2382.17s
- Measured throughput: 41.98 workflows/s
- Peak RSS: 0.84 GiB
- Min system available memory: 13.80 GiB
- Process CPU: peak 350.7% / sample mean 313.0%
- System CPU: peak 86.8% / sample mean 79.8%
- Max storage used: 0.70 GiB
- Peak process rows: 712 (active 16)
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
| warmup | 16 | 100 | 100 | 0 | 0 | 43.55 | 0.0000% | 2.30 |

**warmup start API latency (ms)**: p50 9.216 / p90 22.215 / p95 27.362 / p99 30.292 / p99.9 31.081 / max 31.081 / mean 11.403 / stddev 6.238
**warmup end-to-end latency (ms)**: p50 332.458 / p90 581.598 / p95 635.643 / p99 739.748 / p99.9 826.429 / max 826.429 / mean 342.818 / stddev 175.375

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 41.98 | 0.0000% | 2382.17 |

**load-1/1-c16 start API latency (ms)**: p50 8.472 / p90 11.566 / p95 12.629 / p99 15.016 / p99.9 19.491 / max 84.784 / mean 8.716 / stddev 2.255
**load-1/1-c16 end-to-end latency (ms)**: p50 316.898 / p90 765.160 / p95 909.818 / p99 1169.654 / p99.9 1428.209 / max 1731.544 / mean 381.120 / stddev 266.868

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 39.17 | 0.0000% | 2.55 |

**post-stress-smoke start API latency (ms)**: p50 10.044 / p90 21.804 / p95 25.449 / p99 27.709 / p99.9 27.786 / max 27.786 / mean 11.812 / stddev 5.571
**post-stress-smoke end-to-end latency (ms)**: p50 337.473 / p90 650.211 / p95 747.731 / p99 874.265 / p99.9 921.747 / max 921.747 / mean 372.236 / stddev 192.364

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
