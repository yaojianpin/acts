# Acts million-run stress report

- Generated at (Unix seconds): 1793837925
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 749.20s
- Measured throughput: 133.48 workflows/s
- Peak RSS: 0.36 GiB
- Min system available memory: 14.32 GiB
- Process CPU: peak 380.5% / sample mean 305.1%
- System CPU: peak 95.3% / sample mean 77.7%
- Max storage used: 0.16 GiB
- Peak process rows: 2424 (active 18)
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
| warmup | 16 | 100 | 100 | 0 | 0 | 226.49 | 0.0000% | 0.44 |

**warmup start API latency (ms)**: p50 0.606 / p90 0.898 / p95 2.057 / p99 2.792 / p99.9 14.896 / max 14.896 / mean 0.864 / stddev 1.469
**warmup end-to-end latency (ms)**: p50 60.525 / p90 113.157 / p95 126.610 / p99 161.965 / p99.9 178.595 / max 178.595 / mean 63.839 / stddev 36.749

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 133.48 | 0.0000% | 749.20 |

**load-1/1-c16 start API latency (ms)**: p50 0.811 / p90 1.829 / p95 2.657 / p99 4.331 / p99.9 41.039 / max 136.467 / mean 1.179 / stddev 2.586
**load-1/1-c16 end-to-end latency (ms)**: p50 91.390 / p90 244.737 / p95 314.816 / p99 537.551 / p99.9 806.429 / max 1207.941 / mean 119.861 / stddev 106.509

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 142.87 | 0.0000% | 0.70 |

**post-stress-smoke start API latency (ms)**: p50 0.713 / p90 1.054 / p95 1.156 / p99 1.760 / p99.9 3.266 / max 3.266 / mean 0.803 / stddev 0.314
**post-stress-smoke end-to-end latency (ms)**: p50 81.644 / p90 188.686 / p95 215.031 / p99 244.995 / p99.9 256.440 / max 256.440 / mean 96.031 / stddev 60.793

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
