# Acts million-run stress report

- Generated at (Unix seconds): 1794962755
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 717.61s
- Measured throughput: 139.35 workflows/s
- Peak RSS: 0.29 GiB
- Min system available memory: 14.09 GiB
- Process CPU: peak 385.4% / sample mean 319.0%
- System CPU: peak 96.2% / sample mean 80.6%
- Max storage used: 0.18 GiB
- Peak process rows: 2647 (active 17)
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
| warmup | 16 | 100 | 100 | 0 | 0 | 251.75 | 0.0000% | 0.40 |

**warmup start API latency (ms)**: p50 0.581 / p90 0.982 / p95 1.098 / p99 1.551 / p99.9 3.072 / max 3.072 / mean 0.688 / stddev 0.314
**warmup end-to-end latency (ms)**: p50 58.572 / p90 96.044 / p95 101.454 / p99 116.878 / p99.9 121.627 / max 121.627 / mean 59.628 / stddev 28.211

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 139.35 | 0.0000% | 717.61 |

**load-1/1-c16 start API latency (ms)**: p50 0.817 / p90 1.640 / p95 2.049 / p99 2.866 / p99.9 34.038 / max 149.591 / mean 1.097 / stddev 2.085
**load-1/1-c16 end-to-end latency (ms)**: p50 91.619 / p90 235.288 / p95 291.413 / p99 425.682 / p99.9 595.509 / max 879.452 / mean 114.808 / stddev 91.005

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 142.87 | 0.0000% | 0.70 |

**post-stress-smoke start API latency (ms)**: p50 0.805 / p90 1.168 / p95 1.479 / p99 2.067 / p99.9 6.477 / max 6.477 / mean 0.932 / stddev 0.612
**post-stress-smoke end-to-end latency (ms)**: p50 89.135 / p90 201.683 / p95 214.775 / p99 255.316 / p99.9 269.947 / max 269.947 / mean 101.607 / stddev 71.312

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
