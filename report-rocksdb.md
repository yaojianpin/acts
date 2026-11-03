# Acts million-run stress report

- Generated at (Unix seconds): 1793666060
- Result: **PASS**
- Measured attempts: 100000
- Measured completed: 100000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 2253.40s
- Measured throughput: 44.38 workflows/s
- Peak RSS: 2.49 GiB
- Min system available memory: 12.18 GiB
- Process CPU: peak 374.7% / sample mean 336.4%
- System CPU: peak 92.4% / sample mean 82.8%
- Max storage used: 0.30 GiB
- Peak process rows: 1056 (active 128)
- Process rows at the end (after the cleanup wait, when configured): 0
- Residual aggregate documents: 0
- Residual index documents: 0
- Logical CPUs: 4
- Degraded health samples: 0
- Metric collection failures: 0
- Engine shutdown: 0.100s

## Test configuration

- Workload: workflow: init + variable while-loop + expression evaluation + two-way branch + finalize
- Host: linux / x86_64 (4 logical CPUs)
- Concurrency stages: [16, 32, 64, 128]
- Warmup workflows: 100
- Steady-state soak: 0s
- Cleanup drain wait: 300s
- Scheduler workers: 4
- Store writer workers: 4
- Store shards: 4
- Cache cap: 320
- Scheduler queue cap: 4096
- Store writer queue cap: 4096
- Metric sample interval: 1000ms (each status sample also waits for sysinfo's CPU delta window)
- Per-workflow wait timeout: 120s
- Engine version: 0.1.0
- Store backend: rocksdb (target/stress-results/rocksdb-store, 4 shards)
- Error-rate gate: 0.000%

Backend selected by --store; connection URLs are not copied into this report; closed-loop load; sampled peaks may understate instantaneous peaks. Run a release build on the target machine.

## Per-phase results

| Phase | Concurrency | Attempted | Completed | Errored | Terminated | Throughput/s | Error rate | Elapsed/s |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| warmup | 16 | 100 | 100 | 0 | 0 | 68.16 | 0.0000% | 1.47 |

**warmup start API latency (ms)**: p50 4.643 / p90 9.771 / p95 11.066 / p99 12.150 / p99.9 12.378 / max 12.378 / mean 5.464 / stddev 2.382
**warmup end-to-end latency (ms)**: p50 191.152 / p90 441.946 / p95 485.534 / p99 519.231 / p99.9 529.493 / max 529.493 / mean 219.943 / stddev 133.600

| load-1/4-c16 | 16 | 25000 | 25000 | 0 | 0 | 45.02 | 0.0000% | 555.29 |

**load-1/4-c16 start API latency (ms)**: p50 7.008 / p90 12.919 / p95 14.292 / p99 16.637 / p99.9 20.763 / max 106.192 / mean 8.043 / stddev 3.368
**load-1/4-c16 end-to-end latency (ms)**: p50 283.007 / p90 726.222 / p95 901.284 / p99 1266.548 / p99.9 1621.455 / max 1861.865 / mean 355.281 / stddev 273.593

| load-2/4-c32 | 32 | 25000 | 25000 | 0 | 0 | 43.87 | 0.0000% | 569.87 |

**load-2/4-c32 start API latency (ms)**: p50 8.315 / p90 13.881 / p95 15.107 / p99 17.876 / p99.9 24.030 / max 43.031 / mean 9.040 / stddev 3.474
**load-2/4-c32 end-to-end latency (ms)**: p50 557.590 / p90 1605.194 / p95 1937.662 / p99 2590.371 / p99.9 3273.314 / max 3587.040 / mean 728.784 / stddev 599.826

| load-3/4-c64 | 64 | 25000 | 25000 | 0 | 0 | 44.18 | 0.0000% | 565.88 |

**load-3/4-c64 start API latency (ms)**: p50 8.891 / p90 14.295 / p95 15.680 / p99 19.196 / p99.9 51.264 / max 76.863 / mean 9.497 / stddev 4.037
**load-3/4-c64 end-to-end latency (ms)**: p50 1105.989 / p90 3189.904 / p95 3869.776 / p99 5024.076 / p99.9 5976.742 / max 6410.470 / mean 1447.411 / stddev 1199.182

| load-4/4-c128 | 128 | 25000 | 25000 | 0 | 0 | 44.46 | 0.0000% | 562.36 |

**load-4/4-c128 start API latency (ms)**: p50 9.159 / p90 14.422 / p95 15.735 / p99 20.042 / p99.9 88.407 / max 105.242 / mean 9.871 / stddev 5.526
**load-4/4-c128 end-to-end latency (ms)**: p50 2427.560 / p90 5892.493 / p95 7154.660 / p99 9258.146 / p99.9 11083.504 / max 12172.562 / mean 2870.330 / stddev 2118.358

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 44.17 | 0.0000% | 2.26 |

**post-stress-smoke start API latency (ms)**: p50 8.790 / p90 13.139 / p95 14.607 / p99 17.723 / p99.9 19.543 / max 19.543 / mean 9.301 / stddev 2.831
**post-stress-smoke end-to-end latency (ms)**: p50 303.333 / p90 548.877 / p95 722.924 / p99 846.806 / p99.9 854.776 / max 854.776 / mean 331.917 / stddev 192.168

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
