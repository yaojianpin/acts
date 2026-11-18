# Acts million-run stress report

- Generated at (Unix seconds): 1794963971
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 2244.31s
- Measured throughput: 44.56 workflows/s
- Peak RSS: 0.70 GiB
- Min system available memory: 13.97 GiB
- Process CPU: peak 350.0% / sample mean 316.4%
- System CPU: peak 86.1% / sample mean 79.7%
- Max storage used: 0.71 GiB
- Peak process rows: 720 (active 16)
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
| warmup | 16 | 100 | 100 | 0 | 0 | 47.12 | 0.0000% | 2.12 |

**warmup start API latency (ms)**: p50 7.212 / p90 19.458 / p95 23.415 / p99 29.925 / p99.9 31.698 / max 31.698 / mean 9.706 / stddev 5.905
**warmup end-to-end latency (ms)**: p50 325.798 / p90 543.892 / p95 602.162 / p99 637.825 / p99.9 691.488 / max 691.488 / mean 317.845 / stddev 172.710

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 44.56 | 0.0000% | 2244.31 |

**load-1/1-c16 start API latency (ms)**: p50 8.113 / p90 10.965 / p95 11.876 / p99 13.838 / p99.9 16.980 / max 228.949 / mean 8.311 / stddev 2.171
**load-1/1-c16 end-to-end latency (ms)**: p50 301.163 / p90 718.783 / p95 845.172 / p99 1060.049 / p99.9 1255.933 / max 1496.142 / mean 359.070 / stddev 246.312

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 48.69 | 0.0000% | 2.05 |

**post-stress-smoke start API latency (ms)**: p50 7.880 / p90 20.771 / p95 23.275 / p99 24.392 / p99.9 28.816 / max 28.816 / mean 9.886 / stddev 5.403
**post-stress-smoke end-to-end latency (ms)**: p50 279.740 / p90 545.806 / p95 600.169 / p99 743.677 / p99.9 783.180 / max 783.180 / mean 308.164 / stddev 175.281

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
