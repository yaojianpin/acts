# Acts million-run stress report

- Generated at (Unix seconds): 1794962766
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 1315.85s
- Measured throughput: 76.00 workflows/s
- Peak RSS: 2.25 GiB
- Min system available memory: 12.34 GiB
- Process CPU: peak 353.2% / sample mean 309.3%
- System CPU: peak 89.7% / sample mean 77.2%
- Max storage used: 0.31 GiB
- Peak process rows: 1903 (active 16)
- Process rows at the end (after the cleanup wait, when configured): 0
- Residual aggregate documents: 0
- Residual index documents: 0
- Logical CPUs: 4
- Degraded health samples: 0
- Metric collection failures: 0
- Engine shutdown: 0.301s

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
| warmup | 16 | 100 | 100 | 0 | 0 | 128.40 | 0.0000% | 0.78 |

**warmup start API latency (ms)**: p50 2.372 / p90 4.915 / p95 5.700 / p99 5.975 / p99.9 8.751 / max 8.751 / mean 2.891 / stddev 1.301
**warmup end-to-end latency (ms)**: p50 96.381 / p90 244.626 / p95 283.326 / p99 315.477 / p99.9 336.077 / max 336.077 / mean 116.744 / stddev 82.040

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 76.00 | 0.0000% | 1315.85 |

**load-1/1-c16 start API latency (ms)**: p50 4.010 / p90 7.805 / p95 8.697 / p99 10.704 / p99.9 14.315 / max 189.545 / mean 4.676 / stddev 2.296
**load-1/1-c16 end-to-end latency (ms)**: p50 165.358 / p90 439.773 / p95 543.042 / p99 732.840 / p99.9 922.448 / max 1174.036 / mean 210.519 / stddev 162.845

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 75.38 | 0.0000% | 1.33 |

**post-stress-smoke start API latency (ms)**: p50 4.758 / p90 9.971 / p95 10.304 / p99 11.494 / p99.9 12.657 / max 12.657 / mean 5.678 / stddev 2.568
**post-stress-smoke end-to-end latency (ms)**: p50 194.726 / p90 327.836 / p95 352.182 / p99 407.766 / p99.9 417.271 / max 417.271 / mean 195.879 / stddev 94.083

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
