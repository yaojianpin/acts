# Acts million-run stress report

- Generated at (Unix seconds): 1793883736
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 1592.01s
- Measured throughput: 62.81 workflows/s
- Peak RSS: 2.34 GiB
- Min system available memory: 12.32 GiB
- Process CPU: peak 359.0% / sample mean 312.9%
- System CPU: peak 88.3% / sample mean 76.4%
- Max storage used: 0.29 GiB
- Peak process rows: 1686 (active 16)
- Process rows at the end (after the cleanup wait, when configured): 0
- Residual aggregate documents: 0
- Residual index documents: 0
- Logical CPUs: 4
- Degraded health samples: 0
- Metric collection failures: 0
- Engine shutdown: 0.537s

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
| warmup | 16 | 100 | 100 | 0 | 0 | 112.15 | 0.0000% | 0.89 |

**warmup start API latency (ms)**: p50 2.777 / p90 6.371 / p95 7.392 / p99 8.993 / p99.9 10.200 / max 10.200 / mean 3.574 / stddev 1.827
**warmup end-to-end latency (ms)**: p50 125.505 / p90 219.218 / p95 242.615 / p99 281.722 / p99.9 291.912 / max 291.912 / mean 133.613 / stddev 61.935

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 62.81 | 0.0000% | 1592.01 |

**load-1/1-c16 start API latency (ms)**: p50 4.763 / p90 9.381 / p95 10.366 / p99 12.335 / p99.9 15.732 / max 206.104 / mean 5.588 / stddev 2.753
**load-1/1-c16 end-to-end latency (ms)**: p50 196.811 / p90 542.610 / p95 674.224 / p99 915.044 / p99.9 1140.193 / max 1386.649 / mean 254.706 / stddev 203.054

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 48.76 | 0.0000% | 2.05 |

**post-stress-smoke start API latency (ms)**: p50 7.127 / p90 9.964 / p95 10.855 / p99 13.785 / p99.9 13.966 / max 13.966 / mean 7.174 / stddev 2.565
**post-stress-smoke end-to-end latency (ms)**: p50 201.310 / p90 592.073 / p95 713.725 / p99 800.154 / p99.9 831.150 / max 831.150 / mean 306.342 / stddev 212.477

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
