# Acts million-run stress report

- Generated at (Unix seconds): 1794014984
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 1520.52s
- Measured throughput: 65.77 workflows/s
- Peak RSS: 1.51 GiB
- Min system available memory: 13.14 GiB
- Process CPU: peak 332.4% / sample mean 292.3%
- System CPU: peak 81.1% / sample mean 71.0%
- Max storage used: 1.98 GiB
- Peak process rows: 1365 (active 16)
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
| warmup | 16 | 100 | 100 | 0 | 0 | 91.25 | 0.0000% | 1.10 |

**warmup start API latency (ms)**: p50 4.437 / p90 9.003 / p95 10.538 / p99 11.987 / p99.9 12.834 / max 12.834 / mean 5.233 / stddev 2.261
**warmup end-to-end latency (ms)**: p50 153.609 / p90 252.217 / p95 283.534 / p99 305.473 / p99.9 333.699 / max 333.699 / mean 162.987 / stddev 67.325

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 65.77 | 0.0000% | 1520.52 |

**load-1/1-c16 start API latency (ms)**: p50 5.533 / p90 7.862 / p95 8.642 / p99 10.243 / p99.9 13.386 / max 391.213 / mean 5.734 / stddev 2.213
**load-1/1-c16 end-to-end latency (ms)**: p50 199.707 / p90 493.376 / p95 593.733 / p99 781.292 / p99.9 962.066 / max 1158.368 / mean 243.257 / stddev 175.659

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 58.89 | 0.0000% | 1.70 |

**post-stress-smoke start API latency (ms)**: p50 6.943 / p90 15.249 / p95 16.186 / p99 18.510 / p99.9 18.687 / max 18.687 / mean 8.197 / stddev 3.618
**post-stress-smoke end-to-end latency (ms)**: p50 228.581 / p90 447.214 / p95 527.006 / p99 650.922 / p99.9 653.021 / max 653.021 / mean 259.134 / stddev 142.123

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
