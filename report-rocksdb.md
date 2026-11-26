# Acts million-run stress report

- Generated at (Unix seconds): 1795653571
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 1820.00s
- Measured throughput: 54.95 workflows/s
- Peak RSS: 2.25 GiB
- Min system available memory: 12.38 GiB
- Process CPU: peak 368.0% / sample mean 320.8%
- System CPU: peak 90.7% / sample mean 80.3%
- Max storage used: 0.29 GiB
- Peak process rows: 1336 (active 16)
- Process rows at the end (after the cleanup wait, when configured): 0
- Residual aggregate documents: 0
- Residual index documents: 0
- Logical CPUs: 4
- Degraded health samples: 0
- Metric collection failures: 0
- Engine shutdown: 0.660s

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
| warmup | 16 | 100 | 100 | 0 | 0 | 95.21 | 0.0000% | 1.05 |

**warmup start API latency (ms)**: p50 3.571 / p90 9.271 / p95 12.824 / p99 13.752 / p99.9 15.537 / max 15.537 / mean 4.653 / stddev 2.860
**warmup end-to-end latency (ms)**: p50 152.091 / p90 294.494 / p95 314.429 / p99 335.745 / p99.9 345.380 / max 345.380 / mean 156.544 / stddev 88.757

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 54.95 | 0.0000% | 1820.00 |

**load-1/1-c16 start API latency (ms)**: p50 5.510 / p90 10.377 / p95 11.506 / p99 13.976 / p99.9 18.061 / max 428.965 / mean 6.384 / stddev 3.493
**load-1/1-c16 end-to-end latency (ms)**: p50 225.969 / p90 615.203 / p95 759.955 / p99 1033.440 / p99.9 1290.215 / max 1538.615 / mean 291.168 / stddev 229.020

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 53.59 | 0.0000% | 1.87 |

**post-stress-smoke start API latency (ms)**: p50 6.767 / p90 24.828 / p95 26.307 / p99 29.329 / p99.9 29.782 / max 29.782 / mean 9.251 / stddev 7.209
**post-stress-smoke end-to-end latency (ms)**: p50 225.132 / p90 539.024 / p95 668.793 / p99 800.496 / p99.9 940.845 / max 940.845 / mean 282.359 / stddev 189.844

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
