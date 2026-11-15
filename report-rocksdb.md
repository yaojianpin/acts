# Acts million-run stress report

- Generated at (Unix seconds): 1794729255
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 2477.21s
- Measured throughput: 40.37 workflows/s
- Peak RSS: 2.45 GiB
- Min system available memory: 12.22 GiB
- Process CPU: peak 368.4% / sample mean 321.5%
- System CPU: peak 92.4% / sample mean 81.1%
- Max storage used: 0.27 GiB
- Peak process rows: 1015 (active 16)
- Process rows at the end (after the cleanup wait, when configured): 0
- Residual aggregate documents: 0
- Residual index documents: 0
- Logical CPUs: 4
- Degraded health samples: 0
- Metric collection failures: 0
- Engine shutdown: 0.178s

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
| warmup | 16 | 100 | 100 | 0 | 0 | 69.12 | 0.0000% | 1.45 |

**warmup start API latency (ms)**: p50 4.779 / p90 13.140 / p95 16.619 / p99 18.738 / p99.9 20.792 / max 20.792 / mean 6.405 / stddev 4.130
**warmup end-to-end latency (ms)**: p50 201.222 / p90 374.635 / p95 393.885 / p99 457.194 / p99.9 519.011 / max 519.011 / mean 219.446 / stddev 105.341

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 40.37 | 0.0000% | 2477.21 |

**load-1/1-c16 start API latency (ms)**: p50 7.483 / p90 13.786 / p95 15.312 / p99 18.689 / p99.9 24.692 / max 242.243 / mean 8.587 / stddev 3.748
**load-1/1-c16 end-to-end latency (ms)**: p50 313.285 / p90 823.769 / p95 1025.448 / p99 1395.010 / p99.9 1802.619 / max 2308.702 / mean 396.331 / stddev 306.822

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 50.17 | 0.0000% | 1.99 |

**post-stress-smoke start API latency (ms)**: p50 7.198 / p90 12.055 / p95 13.888 / p99 15.943 / p99.9 16.620 / max 16.620 / mean 8.124 / stddev 2.774
**post-stress-smoke end-to-end latency (ms)**: p50 282.644 / p90 495.811 / p95 528.853 / p99 556.394 / p99.9 564.575 / max 564.575 / mean 297.479 / stddev 129.994

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
