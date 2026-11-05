# Acts million-run stress report

- Generated at (Unix seconds): 1793883215
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 852.68s
- Measured throughput: 117.28 workflows/s
- Peak RSS: 0.31 GiB
- Min system available memory: 14.39 GiB
- Process CPU: peak 381.0% / sample mean 320.4%
- System CPU: peak 97.5% / sample mean 82.0%
- Max storage used: 0.14 GiB
- Peak process rows: 2047 (active 18)
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
| warmup | 16 | 100 | 100 | 0 | 0 | 174.59 | 0.0000% | 0.57 |

**warmup start API latency (ms)**: p50 0.753 / p90 1.483 / p95 2.134 / p99 4.108 / p99.9 22.435 / max 22.435 / mean 1.114 / stddev 2.205
**warmup end-to-end latency (ms)**: p50 83.576 / p90 135.163 / p95 161.512 / p99 173.081 / p99.9 175.173 / max 175.173 / mean 81.945 / stddev 39.881

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 117.28 | 0.0000% | 852.68 |

**load-1/1-c16 start API latency (ms)**: p50 0.931 / p90 1.867 / p95 2.501 / p99 4.012 / p99.9 41.370 / max 136.372 / mean 1.288 / stddev 2.578
**load-1/1-c16 end-to-end latency (ms)**: p50 109.505 / p90 279.061 / p95 345.040 / p99 518.038 / p99.9 731.512 / max 1048.565 / mean 136.415 / stddev 109.220

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 137.61 | 0.0000% | 0.73 |

**post-stress-smoke start API latency (ms)**: p50 0.875 / p90 1.441 / p95 1.504 / p99 2.655 / p99.9 5.442 / max 5.442 / mean 1.026 / stddev 0.540
**post-stress-smoke end-to-end latency (ms)**: p50 103.154 / p90 188.798 / p95 226.393 / p99 251.616 / p99.9 254.843 / max 254.843 / mean 105.316 / stddev 61.906

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
