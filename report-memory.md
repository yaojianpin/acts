# Acts million-run stress report

- Generated at (Unix seconds): 1794660187
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 636.68s
- Measured throughput: 157.06 workflows/s
- Peak RSS: 0.37 GiB
- Min system available memory: 14.31 GiB
- Process CPU: peak 380.2% / sample mean 289.8%
- System CPU: peak 97.6% / sample mean 73.9%
- Max storage used: 0.24 GiB
- Peak process rows: 3463 (active 18)
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
| warmup | 16 | 100 | 100 | 0 | 0 | 320.27 | 0.0000% | 0.31 |

**warmup start API latency (ms)**: p50 0.434 / p90 0.739 / p95 0.978 / p99 3.489 / p99.9 4.399 / max 4.399 / mean 0.583 / stddev 0.553
**warmup end-to-end latency (ms)**: p50 39.216 / p90 89.026 / p95 99.528 / p99 124.490 / p99.9 139.011 / max 139.011 / mean 44.930 / stddev 26.004

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 157.06 | 0.0000% | 636.68 |

**load-1/1-c16 start API latency (ms)**: p50 0.679 / p90 1.785 / p95 2.765 / p99 4.865 / p99.9 36.954 / max 189.121 / mean 1.076 / stddev 2.597
**load-1/1-c16 end-to-end latency (ms)**: p50 71.622 / p90 211.531 / p95 302.505 / p99 537.116 / p99.9 860.348 / max 1177.522 / mean 101.861 / stddev 104.840

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 221.21 | 0.0000% | 0.45 |

**post-stress-smoke start API latency (ms)**: p50 0.587 / p90 0.909 / p95 0.947 / p99 1.281 / p99.9 1.637 / max 1.637 / mean 0.667 / stddev 0.185
**post-stress-smoke end-to-end latency (ms)**: p50 66.513 / p90 108.443 / p95 116.469 / p99 126.363 / p99.9 132.689 / max 132.689 / mean 65.099 / stddev 33.068

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
