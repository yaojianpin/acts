# Acts million-run stress report

- Generated at (Unix seconds): 1793542040
- Result: **PASS**
- Measured attempts: 5000
- Measured completed: 5000
- Measured errored: 0
- Measured terminated: 0
- Measured error rate: 0.0000%
- Measured elapsed: 44.54s
- Measured throughput: 112.27 workflows/s
- Peak RSS: 0.26 GiB
- Min system available memory: 14.50 GiB
- Process CPU: peak 381.6% / sample mean 239.5%
- System CPU: peak 95.4% / sample mean 61.7%
- Max storage used: 0.13 GiB
- Peak process rows: 2004 (active 64)
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
- Concurrency stages: [16, 32, 64]
- Warmup workflows: 100
- Steady-state soak: 0s
- Cleanup drain wait: 300s
- Scheduler workers: 4
- Store writer workers: 4
- Cache cap: 192
- Scheduler queue cap: 2048
- Store writer queue cap: 2048
- Metric sample interval: 1000ms (each status sample also waits for sysinfo's CPU delta window)
- Per-workflow wait timeout: 120s
- Engine version: 0.1.0
- Store backend: memory
- Error-rate gate: 0.000%

Backend selected by --store; connection URLs are not copied into this report; closed-loop load; sampled peaks may understate instantaneous peaks. Run a release build on the target machine.

## Per-phase results

| Phase | Concurrency | Attempted | Completed | Errored | Terminated | Throughput/s | Error rate | Elapsed/s |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| warmup | 16 | 100 | 100 | 0 | 0 | 158.62 | 0.0000% | 0.63 |

**warmup start API latency (ms)**: p50 0.778 / p90 1.352 / p95 1.914 / p99 3.158 / p99.9 3.519 / max 3.519 / mean 0.952 / stddev 0.487
**warmup end-to-end latency (ms)**: p50 45.126 / p90 224.827 / p95 275.855 / p99 302.900 / p99.9 311.175 / max 311.175 / mean 91.314 / stddev 83.758

| load-1/3-c16 | 16 | 1667 | 1667 | 0 | 0 | 136.52 | 0.0000% | 12.21 |

**load-1/3-c16 start API latency (ms)**: p50 0.961 / p90 1.336 / p95 1.546 / p99 2.660 / p99.9 28.973 / max 30.012 / mean 1.084 / stddev 1.220
**load-1/3-c16 end-to-end latency (ms)**: p50 98.664 / p90 232.865 / p95 265.942 / p99 338.802 / p99.9 417.995 / max 444.604 / mean 116.654 / stddev 79.294

| load-2/3-c32 | 32 | 1667 | 1667 | 0 | 0 | 87.93 | 0.0000% | 18.96 |

**load-2/3-c32 start API latency (ms)**: p50 1.126 / p90 3.686 / p95 4.281 / p99 5.182 / p99.9 22.985 / max 57.606 / mean 1.699 / stddev 1.859
**load-2/3-c32 end-to-end latency (ms)**: p50 260.499 / p90 782.688 / p95 1040.696 / p99 1460.697 / p99.9 1682.443 / max 1746.356 / mean 360.457 / stddev 319.320

| load-3/3-c64 | 64 | 1666 | 1666 | 0 | 0 | 124.63 | 0.0000% | 13.37 |

**load-3/3-c64 start API latency (ms)**: p50 0.975 / p90 1.519 / p95 2.087 / p99 3.566 / p99.9 28.872 / max 49.324 / mean 1.179 / stddev 1.474
**load-3/3-c64 end-to-end latency (ms)**: p50 429.784 / p90 1013.591 / p95 1155.129 / p99 1306.545 / p99.9 1423.684 / max 1445.751 / mean 500.569 / stddev 338.885

| post-stress-smoke | 16 | 100 | 100 | 0 | 0 | 49.33 | 0.0000% | 2.03 |

**post-stress-smoke start API latency (ms)**: p50 3.209 / p90 4.547 / p95 4.813 / p99 5.235 / p99.9 5.349 / max 5.349 / mean 2.793 / stddev 1.484
**post-stress-smoke end-to-end latency (ms)**: p50 242.851 / p90 589.918 / p95 717.280 / p99 905.790 / p99.9 931.559 / max 931.559 / mean 305.247 / stddev 215.089

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
