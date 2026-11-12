# Acts million-run stress report

- Generated at (Unix seconds): 1794516947
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 1831.99s
- Measured throughput: 54.59 workflows/s
- Peak RSS: 2.32 GiB
- Min system available memory: 12.25 GiB
- Process CPU: peak 368.0% / sample mean 318.9%
- System CPU: peak 91.1% / sample mean 80.0%
- Max storage used: 0.29 GiB
- Peak process rows: 1380 (active 16)
- Process rows at the end (after the cleanup wait, when configured): 0
- Residual aggregate documents: 0
- Residual index documents: 0
- Logical CPUs: 4
- Degraded health samples: 0
- Metric collection failures: 0
- Engine shutdown: 1.748s

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
| warmup | 16 | 100 | 100 | 0 | 0 | 93.04 | 0.0000% | 1.07 |

**warmup start API latency (ms)**: p50 3.504 / p90 7.956 / p95 10.010 / p99 11.146 / p99.9 13.495 / max 13.495 / mean 4.365 / stddev 2.326
**warmup end-to-end latency (ms)**: p50 159.518 / p90 275.825 / p95 304.376 / p99 323.286 / p99.9 338.684 / max 338.684 / mean 158.618 / stddev 76.967

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 54.59 | 0.0000% | 1831.99 |

**load-1/1-c16 start API latency (ms)**: p50 5.428 / p90 10.457 / p95 11.649 / p99 14.339 / p99.9 19.164 / max 224.057 / mean 6.357 / stddev 3.156
**load-1/1-c16 end-to-end latency (ms)**: p50 227.822 / p90 622.376 / p95 771.817 / p99 1030.842 / p99.9 1279.504 / max 1524.609 / mean 293.087 / stddev 230.817

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 40.11 | 0.0000% | 2.49 |

**post-stress-smoke start API latency (ms)**: p50 8.879 / p90 13.527 / p95 14.799 / p99 21.038 / p99.9 21.541 / max 21.541 / mean 9.151 / stddev 3.452
**post-stress-smoke end-to-end latency (ms)**: p50 286.796 / p90 735.280 / p95 838.307 / p99 951.185 / p99.9 1007.262 / max 1007.262 / mean 363.804 / stddev 256.378

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
