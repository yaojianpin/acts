# Acts million-run stress report

- Generated at (Unix seconds): 1795530024
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 2250.52s
- Measured throughput: 44.43 workflows/s
- Peak RSS: 1.14 GiB
- Min system available memory: 13.19 GiB
- Process CPU: peak 338.9% / sample mean 305.1%
- System CPU: peak 80.8% / sample mean 75.0%
- Max storage used: 1.53 GiB
- Peak process rows: 812 (active 16)
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
| warmup | 16 | 100 | 100 | 0 | 0 | 55.60 | 0.0000% | 1.80 |

**warmup start API latency (ms)**: p50 4.921 / p90 11.308 / p95 12.731 / p99 13.804 / p99.9 13.824 / max 13.824 / mean 6.002 / stddev 2.840
**warmup end-to-end latency (ms)**: p50 225.304 / p90 497.326 / p95 624.728 / p99 737.950 / p99.9 746.062 / max 746.062 / mean 274.286 / stddev 159.569

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 44.43 | 0.0000% | 2250.52 |

**load-1/1-c16 start API latency (ms)**: p50 5.258 / p90 7.090 / p95 7.673 / p99 8.959 / p99.9 10.669 / max 18.256 / mean 5.395 / stddev 1.277
**load-1/1-c16 end-to-end latency (ms)**: p50 303.804 / p90 718.241 / p95 853.892 / p99 1082.631 / p99.9 1274.400 / max 1787.530 / mean 360.053 / stddev 248.513

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 40.12 | 0.0000% | 2.49 |

**post-stress-smoke start API latency (ms)**: p50 6.361 / p90 17.470 / p95 19.597 / p99 20.895 / p99.9 21.371 / max 21.371 / mean 8.122 / stddev 4.542
**post-stress-smoke end-to-end latency (ms)**: p50 305.908 / p90 802.295 / p95 885.765 / p99 1081.171 / p99.9 1142.194 / max 1142.194 / mean 373.806 / stddev 269.396

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
