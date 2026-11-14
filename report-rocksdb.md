# Acts million-run stress report

- Generated at (Unix seconds): 1794629471
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 1738.79s
- Measured throughput: 57.51 workflows/s
- Peak RSS: 2.27 GiB
- Min system available memory: 12.39 GiB
- Process CPU: peak 369.6% / sample mean 317.5%
- System CPU: peak 91.1% / sample mean 79.4%
- Max storage used: 0.30 GiB
- Peak process rows: 1381 (active 16)
- Process rows at the end (after the cleanup wait, when configured): 0
- Residual aggregate documents: 0
- Residual index documents: 0
- Logical CPUs: 4
- Degraded health samples: 0
- Metric collection failures: 0
- Engine shutdown: 0.551s

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
| warmup | 16 | 100 | 100 | 0 | 0 | 81.95 | 0.0000% | 1.22 |

**warmup start API latency (ms)**: p50 3.172 / p90 9.167 / p95 9.991 / p99 11.150 / p99.9 13.811 / max 13.811 / mean 4.174 / stddev 2.557
**warmup end-to-end latency (ms)**: p50 90.786 / p90 449.881 / p95 509.368 / p99 555.788 / p99.9 583.298 / max 583.298 / mean 179.467 / stddev 163.184

| load-1/1-c16 | 16 | 100000 | 100000 | 0 | 0 | 57.51 | 0.0000% | 1738.79 |

**load-1/1-c16 start API latency (ms)**: p50 5.232 / p90 9.864 / p95 10.946 / p99 13.337 / p99.9 18.145 / max 115.574 / mean 6.068 / stddev 2.759
**load-1/1-c16 end-to-end latency (ms)**: p50 218.895 / p90 580.389 / p95 718.399 / p99 976.939 / p99.9 1207.884 / max 1510.786 / mean 278.183 / stddev 215.592

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 52.18 | 0.0000% | 1.92 |

**post-stress-smoke start API latency (ms)**: p50 7.062 / p90 10.938 / p95 11.403 / p99 15.038 / p99.9 16.321 / max 16.321 / mean 7.518 / stddev 2.615
**post-stress-smoke end-to-end latency (ms)**: p50 280.879 / p90 487.901 / p95 572.493 / p99 720.253 / p99.9 752.049 / max 752.049 / mean 290.218 / stddev 159.275

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
