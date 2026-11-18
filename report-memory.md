# Acts million-run stress report

- Generated at (Unix seconds): 1795043563
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 689.17s
- Measured throughput: 145.10 workflows/s
- Peak RSS: 0.30 GiB
- Min system available memory: 14.34 GiB
- Process CPU: peak 380.0% / sample mean 308.8%
- System CPU: peak 96.3% / sample mean 78.9%
- Max storage used: 0.19 GiB
- Peak process rows: 2782 (active 18)
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
- Store shards: 1
- Cache cap: 96
- Scheduler queue cap: 1024
- Store writer queue cap: 1024
- Metric sample interval: 1000ms (each status sample also waits for sysinfo's CPU delta window)
- Per-workflow wait timeout: 120s
- Engine version: 0.1.0
- Store backend: memory
- Error-rate gate: 0.000%

Backend selected by --store; connection URLs are not copied into this report; closed-loop load; sampled peaks may understate instantaneous peaks. Run a release build on the target machine.

## Per-phase results

| Phase | Concurrency | Attempted | Completed | Errored | Terminated | Throughput/s | Error rate | Elapsed/s |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| warmup | 16 | 100 | 100 | 0 | 0 | 237.95 | 0.0000% | 0.42 |

**warmup start API latency (ms)**: p50 0.556 / p90 0.842 / p95 1.281 / p99 2.695 / p99.9 4.438 / max 4.438 / mean 0.681 / stddev 0.479
**warmup end-to-end latency (ms)**: p50 58.811 / p90 106.203 / p95 113.881 / p99 138.840 / p99.9 148.197 / max 148.197 / mean 60.490 / stddev 32.243

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 145.10 | 0.0000% | 689.17 |

**load-1/1-c16 start API latency (ms)**: p50 0.729 / p90 1.710 / p95 2.343 / p99 3.565 / p99.9 33.889 / max 136.893 / mean 1.061 / stddev 2.080
**load-1/1-c16 end-to-end latency (ms)**: p50 85.489 / p90 227.583 / p95 288.786 / p99 455.981 / p99.9 651.317 / max 925.594 / mean 110.261 / stddev 93.365

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 137.41 | 0.0000% | 0.73 |

**post-stress-smoke start API latency (ms)**: p50 0.729 / p90 1.064 / p95 1.199 / p99 1.412 / p99.9 3.579 / max 3.579 / mean 0.811 / stddev 0.331
**post-stress-smoke end-to-end latency (ms)**: p50 89.888 / p90 195.827 / p95 222.643 / p99 248.155 / p99.9 278.946 / max 278.946 / mean 103.140 / stddev 61.329

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
