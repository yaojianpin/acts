# Acts million-run stress report

- Generated at (Unix seconds): 1794962324
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 728.50s
- Measured throughput: 137.27 workflows/s
- Peak RSS: 0.29 GiB
- Min system available memory: 14.39 GiB
- Process CPU: peak 385.0% / sample mean 308.9%
- System CPU: peak 96.6% / sample mean 78.9%
- Max storage used: 0.18 GiB
- Peak process rows: 2659 (active 17)
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
| warmup | 16 | 100 | 100 | 0 | 0 | 224.42 | 0.0000% | 0.45 |

**warmup start API latency (ms)**: p50 0.563 / p90 0.919 / p95 1.269 / p99 2.512 / p99.9 4.413 / max 4.413 / mean 0.706 / stddev 0.507
**warmup end-to-end latency (ms)**: p50 45.483 / p90 137.386 / p95 160.945 / p99 192.415 / p99.9 197.127 / max 197.127 / mean 63.919 / stddev 48.528

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 137.27 | 0.0000% | 728.50 |

**load-1/1-c16 start API latency (ms)**: p50 0.774 / p90 1.827 / p95 2.597 / p99 4.159 / p99.9 39.888 / max 809.463 / mean 1.165 / stddev 4.959
**load-1/1-c16 end-to-end latency (ms)**: p50 88.794 / p90 239.765 / p95 310.206 / p99 511.807 / p99.9 773.338 / max 1250.478 / mean 116.554 / stddev 103.492

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 175.99 | 0.0000% | 0.57 |

**post-stress-smoke start API latency (ms)**: p50 0.758 / p90 1.123 / p95 1.319 / p99 4.092 / p99.9 22.242 / max 22.242 / mean 1.098 / stddev 2.181
**post-stress-smoke end-to-end latency (ms)**: p50 74.280 / p90 154.284 / p95 180.676 / p99 188.478 / p99.9 203.412 / max 203.412 / mean 82.310 / stddev 46.953

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
