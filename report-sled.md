# Acts million-run stress report

- Generated at (Unix seconds): 1795104849
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 3227.52s
- Measured throughput: 30.98 workflows/s
- Peak RSS: 1.11 GiB
- Min system available memory: 13.48 GiB
- Process CPU: peak 350.7% / sample mean 293.3%
- System CPU: peak 84.4% / sample mean 73.3%
- Max storage used: 1.05 GiB
- Peak process rows: 580 (active 16)
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
| warmup | 16 | 100 | 100 | 0 | 0 | 32.33 | 0.0000% | 3.09 |

**warmup start API latency (ms)**: p50 6.787 / p90 18.815 / p95 26.250 / p99 31.172 / p99.9 35.482 / max 35.482 / mean 9.307 / stddev 6.508
**warmup end-to-end latency (ms)**: p50 403.762 / p90 1000.351 / p95 1115.555 / p99 1222.889 / p99.9 1261.742 / max 1261.742 / mean 466.528 / stddev 325.584

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 30.98 | 0.0000% | 3227.52 |

**load-1/1-c16 start API latency (ms)**: p50 7.677 / p90 10.520 / p95 11.482 / p99 13.780 / p99.9 19.291 / max 965.213 / mean 7.937 / stddev 3.756
**load-1/1-c16 end-to-end latency (ms)**: p50 434.861 / p90 1036.167 / p95 1228.509 / p99 1543.375 / p99.9 1810.961 / max 2299.542 / mean 516.376 / stddev 356.896

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 30.07 | 0.0000% | 3.33 |

**post-stress-smoke start API latency (ms)**: p50 8.034 / p90 20.421 / p95 24.307 / p99 26.742 / p99.9 27.342 / max 27.342 / mean 10.001 / stddev 5.744
**post-stress-smoke end-to-end latency (ms)**: p50 496.235 / p90 833.721 / p95 890.097 / p99 1029.639 / p99.9 1038.348 / max 1038.348 / mean 491.608 / stddev 242.121

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
