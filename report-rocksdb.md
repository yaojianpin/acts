# Acts million-run stress report

- Generated at (Unix seconds): 1794964086
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 2306.65s
- Measured throughput: 43.35 workflows/s
- Peak RSS: 2.29 GiB
- Min system available memory: 12.37 GiB
- Process CPU: peak 374.4% / sample mean 322.5%
- System CPU: peak 91.0% / sample mean 81.2%
- Max storage used: 0.29 GiB
- Peak process rows: 1027 (active 16)
- Process rows at the end (after the cleanup wait, when configured): 0
- Residual aggregate documents: 0
- Residual index documents: 0
- Logical CPUs: 4
- Degraded health samples: 0
- Metric collection failures: 0
- Engine shutdown: 0.171s

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
| warmup | 16 | 100 | 100 | 0 | 0 | 71.59 | 0.0000% | 1.40 |

**warmup start API latency (ms)**: p50 4.330 / p90 12.926 / p95 15.363 / p99 17.524 / p99.9 20.348 / max 20.348 / mean 5.798 / stddev 3.951
**warmup end-to-end latency (ms)**: p50 189.352 / p90 392.223 / p95 443.713 / p99 507.825 / p99.9 537.221 / max 537.221 / mean 207.975 / stddev 121.839

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 43.35 | 0.0000% | 2306.65 |

**load-1/1-c16 start API latency (ms)**: p50 6.959 / p90 12.662 / p95 14.033 / p99 17.115 / p99.9 22.305 / max 409.828 / mean 7.961 / stddev 3.540
**load-1/1-c16 end-to-end latency (ms)**: p50 292.043 / p90 766.535 / p95 941.633 / p99 1277.570 / p99.9 1605.779 / max 2020.843 / mean 369.045 / stddev 282.983

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 62.77 | 0.0000% | 1.59 |

**post-stress-smoke start API latency (ms)**: p50 5.583 / p90 11.793 / p95 12.984 / p99 14.085 / p99.9 15.694 / max 15.694 / mean 6.588 / stddev 2.672
**post-stress-smoke end-to-end latency (ms)**: p50 242.687 / p90 411.255 / p95 453.639 / p99 498.420 / p99.9 565.082 / max 565.082 / mean 242.079 / stddev 125.501

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
