# Acts million-run stress report

- Generated at (Unix seconds): 1795102640
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 1105.02s
- Measured throughput: 90.50 workflows/s
- Peak RSS: 0.28 GiB
- Min system available memory: 14.34 GiB
- Process CPU: peak 380.2% / sample mean 306.1%
- System CPU: peak 95.1% / sample mean 77.7%
- Max storage used: 0.17 GiB
- Peak process rows: 1856 (active 17)
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
| warmup | 16 | 100 | 100 | 0 | 0 | 151.46 | 0.0000% | 0.66 |

**warmup start API latency (ms)**: p50 0.803 / p90 1.603 / p95 2.257 / p99 3.088 / p99.9 4.349 / max 4.349 / mean 1.037 / stddev 0.596
**warmup end-to-end latency (ms)**: p50 89.277 / p90 186.532 / p95 204.776 / p99 222.373 / p99.9 236.497 / max 236.497 / mean 94.584 / stddev 55.989

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 90.50 | 0.0000% | 1105.02 |

**load-1/1-c16 start API latency (ms)**: p50 1.144 / p90 2.401 / p95 3.361 / p99 5.431 / p99.9 45.709 / max 120.913 / mean 1.572 / stddev 2.661
**load-1/1-c16 end-to-end latency (ms)**: p50 137.016 / p90 370.653 / p95 466.385 / p99 687.358 / p99.9 949.681 / max 1388.137 / mean 176.790 / stddev 145.657

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 95.25 | 0.0000% | 1.05 |

**post-stress-smoke start API latency (ms)**: p50 1.207 / p90 1.821 / p95 2.165 / p99 8.442 / p99.9 28.876 / max 28.876 / mean 1.648 / stddev 2.866
**post-stress-smoke end-to-end latency (ms)**: p50 109.264 / p90 337.915 / p95 395.989 / p99 501.274 / p99.9 513.567 / max 513.567 / mean 155.031 / stddev 125.851

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
