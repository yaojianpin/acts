# Acts million-run stress report

- Generated at (Unix seconds): 1793804430
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 2578.35s
- Measured throughput: 38.78 workflows/s
- Peak RSS: 6.57 GiB
- Min system available memory: 8.10 GiB
- Process CPU: peak 373.7% / sample mean 328.8%
- System CPU: peak 90.0% / sample mean 80.7%
- Max storage used: 0.76 GiB
- Peak process rows: 1498 (active 16)
- Process rows at the end (after the cleanup wait, when configured): 0
- Residual aggregate documents: 0
- Residual index documents: 0
- Logical CPUs: 4
- Degraded health samples: 0
- Metric collection failures: 0
- Engine shutdown: 0.839s

## Test configuration

- Workload: workflow: init + variable while-loop + expression evaluation + two-way branch + finalize
- Host: linux / x86_64 (4 logical CPUs)
- Concurrency stages: [16]
- Warmup workflows: 100
- Steady-state soak: 0s
- Cleanup drain wait: 300s
- Scheduler workers: 4
- Store shards: 16
- Cache cap: 96
- Scheduler queue cap: 1024
- Store writer queue cap: 1024
- Metric sample interval: 1000ms (each status sample also waits for sysinfo's CPU delta window)
- Per-workflow wait timeout: 120s
- Engine version: 0.1.0
- Store backend: rocksdb (target/stress-results/rocksdb-baseline-store, 16 shards)
- Error-rate gate: 0.000%

Backend selected by --store; connection URLs are not copied into this report; closed-loop load; sampled peaks may understate instantaneous peaks. Run a release build on the target machine.

## Per-phase results

| Phase | Concurrency | Attempted | Completed | Errored | Terminated | Throughput/s | Error rate | Elapsed/s |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| warmup | 16 | 100 | 100 | 0 | 0 | 100.82 | 0.0000% | 0.99 |

**warmup start API latency (ms)**: p50 4.704 / p90 12.334 / p95 15.076 / p99 17.407 / p99.9 18.386 / max 18.386 / mean 6.415 / stddev 3.754
**warmup end-to-end latency (ms)**: p50 129.996 / p90 266.468 / p95 324.080 / p99 371.381 / p99.9 388.644 / max 388.644 / mean 147.622 / stddev 88.435

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 38.78 | 0.0000% | 2578.35 |

**load-1/1-c16 start API latency (ms)**: p50 9.529 / p90 29.839 / p95 33.660 / p99 39.809 / p99.9 49.163 / max 140.056 / mean 15.217 / stddev 9.982
**load-1/1-c16 end-to-end latency (ms)**: p50 279.033 / p90 964.899 / p95 1260.719 / p99 1820.192 / p99.9 2408.150 / max 2983.733 / mean 412.443 / stddev 394.439

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 20.57 | 0.0000% | 4.86 |

**post-stress-smoke start API latency (ms)**: p50 24.880 / p90 36.430 / p95 42.102 / p99 67.870 / p99.9 69.657 / max 69.657 / mean 25.527 / stddev 10.830
**post-stress-smoke end-to-end latency (ms)**: p50 540.128 / p90 1502.391 / p95 1636.073 / p99 1834.742 / p99.9 1848.902 / max 1848.902 / mean 677.472 / stddev 488.090

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
