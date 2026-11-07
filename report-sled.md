# Acts million-run stress report

- Generated at (Unix seconds): 1794047253
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 2293.71s
- Measured throughput: 43.60 workflows/s
- Peak RSS: 0.75 GiB
- Min system available memory: 13.90 GiB
- Process CPU: peak 352.0% / sample mean 316.4%
- System CPU: peak 86.2% / sample mean 79.9%
- Max storage used: 0.65 GiB
- Peak process rows: 714 (active 16)
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
| warmup | 16 | 100 | 100 | 0 | 0 | 47.54 | 0.0000% | 2.10 |

**warmup start API latency (ms)**: p50 8.513 / p90 22.301 / p95 26.251 / p99 31.495 / p99.9 32.468 / max 32.468 / mean 11.120 / stddev 6.545
**warmup end-to-end latency (ms)**: p50 300.654 / p90 492.688 / p95 552.364 / p99 626.433 / p99.9 665.512 / max 665.512 / mean 319.067 / stddev 128.301

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 43.60 | 0.0000% | 2293.71 |

**load-1/1-c16 start API latency (ms)**: p50 8.143 / p90 10.932 / p95 11.843 / p99 13.629 / p99.9 16.144 / max 205.806 / mean 8.321 / stddev 2.078
**load-1/1-c16 end-to-end latency (ms)**: p50 304.773 / p90 741.915 / p95 875.302 / p99 1107.370 / p99.9 1310.343 / max 1471.766 / mean 366.955 / stddev 256.499

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 36.73 | 0.0000% | 2.72 |

**post-stress-smoke start API latency (ms)**: p50 8.558 / p90 21.607 / p95 23.427 / p99 26.645 / p99.9 27.696 / max 27.696 / mean 10.567 / stddev 5.549
**post-stress-smoke end-to-end latency (ms)**: p50 266.800 / p90 976.496 / p95 1133.004 / p99 1257.446 / p99.9 1341.817 / max 1341.817 / mean 403.940 / stddev 353.773

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
