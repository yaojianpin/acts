# Acts million-run stress report

- Generated at (Unix seconds): 1795044449
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 1641.41s
- Measured throughput: 60.92 workflows/s
- Peak RSS: 0.76 GiB
- Min system available memory: 13.85 GiB
- Process CPU: peak 330.7% / sample mean 290.5%
- System CPU: peak 82.9% / sample mean 71.5%
- Max storage used: 0.92 GiB
- Peak process rows: 1006 (active 16)
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
| warmup | 16 | 100 | 100 | 0 | 0 | 71.82 | 0.0000% | 1.39 |

**warmup start API latency (ms)**: p50 4.792 / p90 16.566 / p95 20.002 / p99 21.736 / p99.9 21.805 / max 21.805 / mean 6.917 / stddev 5.242
**warmup end-to-end latency (ms)**: p50 179.433 / p90 387.709 / p95 432.158 / p99 502.447 / p99.9 516.017 / max 516.017 / mean 207.615 / stddev 124.313

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 60.92 | 0.0000% | 1641.41 |

**load-1/1-c16 start API latency (ms)**: p50 5.530 / p90 7.566 / p95 8.242 / p99 9.666 / p99.9 12.115 / max 361.081 / mean 5.703 / stddev 1.927
**load-1/1-c16 end-to-end latency (ms)**: p50 218.971 / p90 526.324 / p95 626.185 / p99 806.042 / p99.9 980.673 / max 1191.619 / mean 262.606 / stddev 183.333

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 71.07 | 0.0000% | 1.41 |

**post-stress-smoke start API latency (ms)**: p50 5.216 / p90 12.149 / p95 13.865 / p99 16.696 / p99.9 16.865 / max 16.865 / mean 6.373 / stddev 3.105
**post-stress-smoke end-to-end latency (ms)**: p50 203.128 / p90 344.397 / p95 397.185 / p99 432.941 / p99.9 452.837 / max 452.837 / mean 207.669 / stddev 96.093

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
