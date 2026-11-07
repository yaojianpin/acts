# Acts million-run stress report

- Generated at (Unix seconds): 1794014159
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 672.19s
- Measured throughput: 148.77 workflows/s
- Peak RSS: 0.43 GiB
- Min system available memory: 14.27 GiB
- Process CPU: peak 374.4% / sample mean 297.1%
- System CPU: peak 94.9% / sample mean 75.4%
- Max storage used: 0.22 GiB
- Peak process rows: 3308 (active 18)
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
| warmup | 16 | 100 | 100 | 0 | 0 | 319.13 | 0.0000% | 0.31 |

**warmup start API latency (ms)**: p50 0.488 / p90 0.726 / p95 0.940 / p99 3.098 / p99.9 10.424 / max 10.424 / mean 0.680 / stddev 1.037
**warmup end-to-end latency (ms)**: p50 38.004 / p90 90.602 / p95 97.899 / p99 124.745 / p99.9 126.602 / max 126.602 / mean 45.798 / stddev 27.157

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 148.77 | 0.0000% | 672.19 |

**load-1/1-c16 start API latency (ms)**: p50 0.758 / p90 1.944 / p95 2.790 / p99 4.669 / p99.9 37.124 / max 117.692 / mean 1.143 / stddev 2.304
**load-1/1-c16 end-to-end latency (ms)**: p50 76.743 / p90 226.950 / p95 315.174 / p99 528.493 / p99.9 803.075 / max 1154.281 / mean 107.544 / stddev 105.376

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 186.74 | 0.0000% | 0.54 |

**post-stress-smoke start API latency (ms)**: p50 0.838 / p90 1.162 / p95 1.474 / p99 2.205 / p99.9 18.608 / max 18.608 / mean 1.054 / stddev 1.783
**post-stress-smoke end-to-end latency (ms)**: p50 53.049 / p90 177.487 / p95 202.568 / p99 218.346 / p99.9 232.290 / max 232.290 / mean 77.605 / stddev 61.052

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
